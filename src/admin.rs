use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

use crate::server::AppState;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminRequest {
    Pair,
    Revoke,
    Status,
}

#[derive(Debug, Serialize)]
struct AdminResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<crate::model::RemoteState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub async fn run(path: std::path::PathBuf, state: Arc<AppState>) -> Result<()> {
    prepare_socket_path(&path).await?;
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("failed to bind local admin socket {}", path.display()))?;
    secure_socket(&path).await?;
    tracing::info!(path = %path.display(), "local admin socket ready");

    loop {
        let (stream, _) = listener.accept().await.context("admin socket stopped")?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = handle(stream, state).await {
                tracing::debug!(error = %error, "local admin request failed");
            }
        });
    }
}

pub async fn request(path: &Path, request: AdminRequest) -> Result<Value> {
    let mut stream = UnixStream::connect(path).await.with_context(|| {
        format!(
            "could not connect to {}; is CouchMote running?",
            path.display()
        )
    })?;
    let line = serde_json::to_string(&request).context("failed to serialize admin request")?;
    stream.write_all(line.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response).await?;
    let value: Value = serde_json::from_str(&response).context("invalid admin response")?;
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!(
            "{}",
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("admin request failed")
        ));
    }
    Ok(value)
}

pub async fn remove_socket(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
}

async fn handle(stream: UnixStream, state: Arc<AppState>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut line = String::new();
    BufReader::new(reader).read_line(&mut line).await?;
    let request: AdminRequest = serde_json::from_str(&line).context("invalid admin request")?;
    let response = match request {
        AdminRequest::Pair => match state.auth.issue_pairing_code().await {
            Ok(pairing) => AdminResponse {
                ok: true,
                code: Some(pairing.code),
                expires_at: Some(pairing.expires_at),
                state: None,
                error: None,
            },
            Err(error) => failure(error),
        },
        AdminRequest::Revoke => match state.auth.revoke_all().await {
            Ok(()) => AdminResponse {
                ok: true,
                code: None,
                expires_at: None,
                state: None,
                error: None,
            },
            Err(error) => failure(error),
        },
        AdminRequest::Status => AdminResponse {
            ok: true,
            code: None,
            expires_at: None,
            state: Some(state.snapshot().await),
            error: None,
        },
    };
    let serialized = serde_json::to_string(&response)?;
    writer.write_all(serialized.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    Ok(())
}

fn failure(error: anyhow::Error) -> AdminResponse {
    AdminResponse {
        ok: false,
        code: None,
        expires_at: None,
        state: None,
        error: Some(error.to_string()),
    }
}

async fn prepare_socket_path(path: &Path) -> Result<()> {
    if let Ok(metadata) = tokio::fs::symlink_metadata(path).await {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            if !metadata.file_type().is_socket() {
                return Err(anyhow!("refusing to replace non-socket {}", path.display()));
            }
        }
        tokio::fs::remove_file(path)
            .await
            .with_context(|| format!("failed to remove stale socket {}", path.display()))?;
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(())
}

async fn secure_socket(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}
