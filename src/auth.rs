use anyhow::{Context, Result, anyhow};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::IpAddr,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

const PAIRING_TTL_SECONDS: i64 = 10 * 60;
const SESSION_TTL_SECONDS: i64 = 30 * 24 * 60 * 60;
const FAILED_ATTEMPT_WINDOW_SECONDS: i64 = 5 * 60;
const MAX_FAILED_ATTEMPTS: u32 = 8;
pub const SESSION_COOKIE: &str = "couchmote_session";

#[derive(Debug, Clone, Serialize)]
pub struct PairingCode {
    pub code: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSession {
    token_hash: String,
    created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedAuth {
    sessions: Vec<PersistedSession>,
}

#[derive(Debug, Default)]
struct AuthState {
    pending_hash: Option<String>,
    pending_expires_at: i64,
    sessions: Vec<PersistedSession>,
    failures: HashMap<IpAddr, AttemptWindow>,
}

#[derive(Debug, Clone, Copy)]
struct AttemptWindow {
    started_at: i64,
    count: u32,
}

pub struct AuthStore {
    path: PathBuf,
    state: Mutex<AuthState>,
}

impl AuthStore {
    pub async fn load(path: PathBuf) -> Result<Self> {
        let persisted = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice::<PersistedAuth>(&bytes)
                .with_context(|| format!("failed to parse {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => PersistedAuth::default(),
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()));
            }
        };

        let now = now_seconds();
        let sessions = persisted
            .sessions
            .into_iter()
            .filter(|session| now.saturating_sub(session.created_at) < SESSION_TTL_SECONDS)
            .collect();

        Ok(Self {
            path,
            state: Mutex::new(AuthState {
                sessions,
                ..AuthState::default()
            }),
        })
    }

    pub async fn issue_pairing_code(&self) -> Result<PairingCode> {
        let code = format!("{:08}", rand::rng().random_range(0..100_000_000u32));
        let expires_at = now_seconds() + PAIRING_TTL_SECONDS;
        let mut state = self.state.lock().await;
        state.pending_hash = Some(hash_secret(&code));
        state.pending_expires_at = expires_at;
        Ok(PairingCode { code, expires_at })
    }

    pub async fn consume_pairing(&self, ip: IpAddr, code: &str) -> Result<String> {
        let now = now_seconds();
        let mut state = self.state.lock().await;

        let blocked = {
            let attempt = state.failures.entry(ip).or_insert(AttemptWindow {
                started_at: now,
                count: 0,
            });
            if now.saturating_sub(attempt.started_at) >= FAILED_ATTEMPT_WINDOW_SECONDS {
                *attempt = AttemptWindow {
                    started_at: now,
                    count: 0,
                };
            }
            attempt.count >= MAX_FAILED_ATTEMPTS
        };
        if blocked {
            return Err(anyhow!("too many pairing attempts; try again later"));
        }

        let valid = state.pending_expires_at > now
            && state
                .pending_hash
                .as_deref()
                .map(|expected| {
                    constant_time_equal(expected.as_bytes(), hash_secret(code).as_bytes())
                })
                .unwrap_or(false);
        if !valid {
            if let Some(attempt) = state.failures.get_mut(&ip) {
                attempt.count += 1;
            }
            return Err(anyhow!("invalid or expired pairing code"));
        }

        let token = random_token();
        state.sessions.push(PersistedSession {
            token_hash: hash_secret(&token),
            created_at: now,
        });
        state.pending_hash = None;
        state.pending_expires_at = 0;
        state.failures.remove(&ip);
        let persisted = PersistedAuth {
            sessions: state.sessions.clone(),
        };
        drop(state);
        self.persist(&persisted).await?;
        Ok(token)
    }

    pub async fn authenticate(&self, token: &str) -> bool {
        let now = now_seconds();
        let mut state = self.state.lock().await;
        state
            .sessions
            .retain(|session| now.saturating_sub(session.created_at) < SESSION_TTL_SECONDS);
        let expected = hash_secret(token);
        state
            .sessions
            .iter()
            .any(|session| constant_time_equal(session.token_hash.as_bytes(), expected.as_bytes()))
    }

    pub async fn revoke_all(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        state.sessions.clear();
        state.pending_hash = None;
        state.pending_expires_at = 0;
        state.failures.clear();
        let persisted = PersistedAuth::default();
        drop(state);
        self.persist(&persisted).await
    }

    async fn persist(&self, value: &PersistedAuth) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(value).context("failed to serialize auth state")?;
        let temporary = self.path.with_extension("json.tmp");
        tokio::fs::write(&temporary, bytes)
            .await
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        tokio::fs::rename(&temporary, &self.path)
            .await
            .with_context(|| format!("failed to replace {}", self.path.display()))?;
        secure_file(&self.path).await?;
        Ok(())
    }
}

pub fn session_cookie(token: &str) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_TTL_SECONDS}"
    )
}

pub fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

pub fn session_from_cookie(header: Option<&str>) -> Option<&str> {
    header?.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == SESSION_COOKIE).then_some(value)
    })
}

fn hash_secret(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    hex_encode(&digest)
}

fn random_token() -> String {
    let bytes: [u8; 32] = rand::random();
    hex_encode(&bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

async fn secure_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .await
            .with_context(|| format!("failed to secure {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AuthStore, clear_session_cookie, session_cookie, session_from_cookie};
    use std::{
        net::IpAddr,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_path(name: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "couchmote-{name}-{}-{stamp}.json",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn pairing_is_one_time_and_persists_a_session() {
        let path = temp_path("auth");
        let store = AuthStore::load(path.clone())
            .await
            .expect("store should load");
        let pairing = store.issue_pairing_code().await.expect("code should issue");
        let ip: IpAddr = "100.64.0.10".parse().expect("IP should parse");
        let token = store
            .consume_pairing(ip, &pairing.code)
            .await
            .expect("code should pair");
        assert!(store.authenticate(&token).await);
        assert!(store.consume_pairing(ip, &pairing.code).await.is_err());

        let loaded = AuthStore::load(path.clone())
            .await
            .expect("store should reload");
        assert!(loaded.authenticate(&token).await);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[test]
    fn cookie_helpers_round_trip() {
        let cookie = session_cookie("token");
        assert_eq!(session_from_cookie(Some(&cookie)), Some("token"));
        assert!(clear_session_cookie().contains("Max-Age=0"));
    }
}
