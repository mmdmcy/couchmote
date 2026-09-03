use anyhow::{Context, Result, anyhow};
use std::{env, net::IpAddr, path::PathBuf};

use crate::network;

const DEFAULT_PORT: u16 = 8791;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenMode {
    Tailnet,
    Loopback,
}

impl ListenMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "tailnet" | "tailscale" | "private" => Ok(Self::Tailnet),
            "loopback" | "local" => Ok(Self::Loopback),
            other => Err(anyhow!(
                "invalid COUCHMOTE_LISTEN value {other:?}; use tailnet or loopback"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub listen: ListenMode,
    pub browser: String,
    pub state_dir: PathBuf,
    pub profile_dir: PathBuf,
    pub runtime_socket: PathBuf,
}

impl Config {
    pub fn load() -> Result<Self> {
        let state_dir = env::var_os("COUCHMOTE_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_state_dir);
        let profile_dir = env::var_os("COUCHMOTE_PROFILE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| state_dir.join("firefox-profile"));
        let runtime_socket = env::var_os("COUCHMOTE_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(default_runtime_socket);

        let port = env::var("COUCHMOTE_PORT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                value
                    .parse::<u16>()
                    .with_context(|| format!("invalid COUCHMOTE_PORT value {value:?}"))
            })
            .transpose()?
            .unwrap_or(DEFAULT_PORT);

        let listen = ListenMode::parse(
            &env::var("COUCHMOTE_LISTEN").unwrap_or_else(|_| "tailnet".to_string()),
        )?;

        Ok(Self {
            port,
            listen,
            browser: env::var("COUCHMOTE_BROWSER").unwrap_or_else(|_| "firefox".to_string()),
            state_dir,
            profile_dir,
            runtime_socket,
        })
    }

    pub async fn ensure_directories(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.state_dir)
            .await
            .with_context(|| format!("failed to create {}", self.state_dir.display()))?;
        tokio::fs::create_dir_all(&self.profile_dir)
            .await
            .with_context(|| format!("failed to create {}", self.profile_dir.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&self.state_dir, std::fs::Permissions::from_mode(0o700))
                .await
                .with_context(|| format!("failed to secure {}", self.state_dir.display()))?;
            tokio::fs::set_permissions(&self.profile_dir, std::fs::Permissions::from_mode(0o700))
                .await
                .with_context(|| format!("failed to secure {}", self.profile_dir.display()))?;
        }

        Ok(())
    }

    pub fn setup_complete_path(&self) -> PathBuf {
        self.state_dir.join("setup-complete")
    }

    pub async fn setup_complete(&self) -> Result<bool> {
        match tokio::fs::metadata(self.setup_complete_path()).await {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).with_context(|| {
                format!("failed to inspect {}", self.setup_complete_path().display())
            }),
        }
    }

    pub async fn mark_setup_complete(&self) -> Result<()> {
        self.ensure_directories().await?;
        let path = self.setup_complete_path();
        tokio::fs::write(&path, b"CouchMote setup completed.\n")
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .await
                .with_context(|| format!("failed to secure {}", path.display()))?;
        }
        Ok(())
    }

    pub fn autostart_path(&self) -> PathBuf {
        let config_home = env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| PathBuf::from(".config"));
        config_home.join("autostart/couchmote.desktop")
    }

    pub fn sessions_path(&self) -> PathBuf {
        self.state_dir.join("sessions.json")
    }

    pub fn allowed_remote(&self, ip: IpAddr) -> bool {
        ip.is_loopback() || (self.listen == ListenMode::Tailnet && network::is_tailscale_ip(ip))
    }
}

fn default_state_dir() -> PathBuf {
    if let Some(path) = env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(path).join("couchmote");
    }

    dirs_home()
        .map(|home| home.join(".local/state/couchmote"))
        .unwrap_or_else(|| PathBuf::from("data"))
}

fn default_runtime_socket() -> PathBuf {
    if let Some(path) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(path).join("couchmote.sock");
    }

    default_state_dir().join("couchmote.sock")
}

fn dirs_home() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}
