use anyhow::{Context, Result, anyhow};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
    sync::{RwLock, mpsc, oneshot},
    time::{self, MissedTickBehavior},
};

use crate::{
    bidi::{BidiClient, KeyStroke},
    model::{BrowserAction, BrowserSnapshot, BrowserStatus, MediaState},
    youtube::{self, PageProbe},
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Debug, Clone)]
pub struct BrowserConfig {
    pub binary: String,
    pub profile_dir: PathBuf,
}

#[derive(Clone)]
pub struct BrowserHandle {
    tx: mpsc::Sender<BrowserCommand>,
    state: Arc<RwLock<BrowserSnapshot>>,
}

enum BrowserCommand {
    Search {
        query: String,
        response: oneshot::Sender<CommandResult>,
    },
    OpenUrl {
        url: String,
        response: oneshot::Sender<CommandResult>,
    },
    Action {
        action: BrowserAction,
        response: oneshot::Sender<CommandResult>,
    },
}

type CommandResult = Result<(), String>;

impl BrowserHandle {
    pub async fn start(config: BrowserConfig) -> Self {
        let (tx, rx) = mpsc::channel(32);
        let state = Arc::new(RwLock::new(BrowserSnapshot::default()));
        let manager_state = state.clone();
        tokio::spawn(async move {
            let mut manager = BrowserManager {
                config,
                state: manager_state,
                child: None,
                bidi: None,
                context: None,
            };
            if let Err(error) = manager.ensure_started().await {
                manager.set_error(error).await;
            }
            manager.run(rx).await;
        });

        Self { tx, state }
    }

    pub async fn snapshot(&self) -> BrowserSnapshot {
        self.state.read().await.clone()
    }

    pub async fn resolve_result(&self, id: &str) -> Option<String> {
        self.state
            .read()
            .await
            .search_results
            .iter()
            .find(|result| result.id == id)
            .map(|result| result.url.clone())
    }

    pub async fn search(&self, query: String) -> Result<()> {
        self.send(|response| BrowserCommand::Search { query, response })
            .await
    }

    pub async fn open_url(&self, url: String) -> Result<()> {
        self.send(|response| BrowserCommand::OpenUrl { url, response })
            .await
    }

    pub async fn action(&self, action: BrowserAction) -> Result<()> {
        self.send(|response| BrowserCommand::Action { action, response })
            .await
    }

    async fn send<F>(&self, make_command: F) -> Result<()>
    where
        F: FnOnce(oneshot::Sender<CommandResult>) -> BrowserCommand,
    {
        let (response, result) = oneshot::channel();
        self.tx
            .send(make_command(response))
            .await
            .map_err(|_| anyhow!("browser manager is not running"))?;
        result
            .await
            .map_err(|_| anyhow!("browser manager stopped"))?
            .map_err(|error| anyhow!(error))
    }
}

struct BrowserManager {
    config: BrowserConfig,
    state: Arc<RwLock<BrowserSnapshot>>,
    child: Option<Child>,
    bidi: Option<BidiClient>,
    context: Option<String>,
}

impl BrowserManager {
    async fn run(&mut self, mut rx: mpsc::Receiver<BrowserCommand>) {
        let mut ticker = time::interval(Duration::from_secs(4));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                command = rx.recv() => {
                    let Some(command) = command else { break };
                    self.handle_command(command).await;
                }
                _ = ticker.tick() => {
                    self.monitor().await;
                }
            }
        }

        self.shutdown_current().await;
    }

    async fn handle_command(&mut self, command: BrowserCommand) {
        let (result, response) = match command {
            BrowserCommand::Search { query, response } => {
                (self.search_impl(&query).await, response)
            }
            BrowserCommand::OpenUrl { url, response } => (self.open_url_impl(&url).await, response),
            BrowserCommand::Action { action, response } => {
                (self.action_impl(action).await, response)
            }
        };

        if let Err(error) = &result {
            self.set_error(anyhow!("{error}")).await;
        }
        let _ = response.send(result.map_err(|error| format!("{error:#}")));
    }

    async fn ensure_started(&mut self) -> Result<()> {
        if self.child_is_alive().await? && self.bidi.is_some() && self.context.is_some() {
            return Ok(());
        }

        self.shutdown_current().await;
        self.set_status(BrowserStatus::Starting, None).await;

        tokio::fs::create_dir_all(&self.config.profile_dir)
            .await
            .with_context(|| format!("failed to create {}", self.config.profile_dir.display()))?;

        let mut command = Command::new(&self.config.binary);
        command
            .args([
                "--no-remote",
                "--new-instance",
                "--profile",
                self.config.profile_dir.to_string_lossy().as_ref(),
                "--kiosk",
                "--remote-debugging-port=0",
                youtube::YOUTUBE_HOME,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start Firefox using {}", self.config.binary))?;
        let (endpoint_tx, mut endpoint_rx) = mpsc::channel(2);
        if let Some(stdout) = child.stdout.take() {
            spawn_output_reader(stdout, endpoint_tx.clone(), "firefox-stdout");
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_output_reader(stderr, endpoint_tx, "firefox-stderr");
        }

        let endpoint = match time::timeout(STARTUP_TIMEOUT, endpoint_rx.recv()).await {
            Ok(Some(endpoint)) => endpoint,
            Ok(None) => return Err(anyhow!("Firefox closed before exposing WebDriver BiDi")),
            Err(_) => return Err(anyhow!("timed out waiting for Firefox WebDriver BiDi")),
        };

        let mut bidi = match BidiClient::connect(&endpoint).await {
            Ok(bidi) => bidi,
            Err(error) => {
                let _ = child.kill().await;
                return Err(error);
            }
        };
        let context = match bidi.create_session().await {
            Ok(context) => context,
            Err(error) => {
                let _ = child.kill().await;
                return Err(error);
            }
        };
        self.child = Some(child);
        self.bidi = Some(bidi);
        self.context = Some(context);
        self.refresh().await?;
        self.set_status(BrowserStatus::Ready, None).await;
        Ok(())
    }

    async fn search_impl(&mut self, query: &str) -> Result<()> {
        let query = query.trim();
        if query.is_empty() {
            return Err(anyhow!("search query cannot be empty"));
        }
        if query.chars().count() > 200 {
            return Err(anyhow!("search query is limited to 200 characters"));
        }

        self.ensure_started().await?;
        let context = self.context()?.to_string();
        let url = youtube::youtube_search_url(query);
        self.bidi_mut()?.navigate(&context, &url).await?;

        for _ in 0..16 {
            time::sleep(Duration::from_millis(250)).await;
            if self.refresh().await.is_ok() {
                let snapshot = self.state.read().await.clone();
                if snapshot.page_kind.as_deref() == Some("search")
                    && !snapshot.search_results.is_empty()
                {
                    break;
                }
            }
        }
        Ok(())
    }

    async fn open_url_impl(&mut self, url: &str) -> Result<()> {
        let url = youtube::validate_youtube_watch_url(url)?;
        self.ensure_started().await?;
        let context = self.context()?.to_string();
        self.bidi_mut()?.navigate(&context, &url).await?;
        time::sleep(Duration::from_millis(250)).await;
        self.refresh().await?;
        Ok(())
    }

    async fn action_impl(&mut self, action: BrowserAction) -> Result<()> {
        if matches!(action, BrowserAction::Launch) {
            return self.ensure_started().await;
        }
        self.ensure_started().await?;
        let context = self.context()?.to_string();
        let _ = self.bidi_mut()?.activate(&context).await;

        match action {
            BrowserAction::Launch => unreachable!(),
            BrowserAction::PlayPause => {
                self.key_press(&context, vec![KeyStroke::press("k")])
                    .await?;
            }
            BrowserAction::Seek { seconds } => {
                let script = youtube::seek_script(seconds.clamp(-60, 60));
                let _ = self.bidi_mut()?.evaluate_text(&context, &script).await?;
            }
            BrowserAction::Next => {
                self.shifted_key(&context, "n").await?;
            }
            BrowserAction::Previous => {
                self.shifted_key(&context, "p").await?;
            }
            BrowserAction::Fullscreen => {
                self.key_press(&context, vec![KeyStroke::press("f")])
                    .await?;
            }
            BrowserAction::Back => {
                self.bidi_mut()?.traverse_history(&context, -1).await?;
            }
            BrowserAction::Home => {
                self.bidi_mut()?
                    .navigate(&context, youtube::YOUTUBE_HOME)
                    .await?;
            }
            BrowserAction::NavigateUp => {
                self.key_press(&context, vec![KeyStroke::press("\u{e013}")])
                    .await?
            }
            BrowserAction::NavigateDown => {
                self.key_press(&context, vec![KeyStroke::press("\u{e015}")])
                    .await?
            }
            BrowserAction::NavigateLeft => {
                self.key_press(&context, vec![KeyStroke::press("\u{e012}")])
                    .await?
            }
            BrowserAction::NavigateRight => {
                self.key_press(&context, vec![KeyStroke::press("\u{e014}")])
                    .await?
            }
            BrowserAction::NavigateSelect => {
                self.key_press(&context, vec![KeyStroke::press("\u{e007}")])
                    .await?
            }
        }

        time::sleep(Duration::from_millis(180)).await;
        let _ = self.refresh().await;
        Ok(())
    }

    async fn refresh(&mut self) -> Result<()> {
        let context = self.context()?.to_string();
        let probe: PageProbe = self
            .bidi_mut()?
            .evaluate_json(&context, youtube::STATUS_SCRIPT)
            .await?;
        let search_results = if probe.page_kind == "search" {
            match self
                .bidi_mut()?
                .evaluate_text(&context, youtube::SEARCH_SCRIPT)
                .await
            {
                Ok(raw) => youtube::parse_search_results(&raw).unwrap_or_default(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let media = probe
            .media
            .map_or_else(MediaState::default, |media| MediaState {
                available: media.available,
                playing: media.playing,
                position_seconds: finite(media.position_seconds),
                duration_seconds: finite(media.duration_seconds),
                title: media.title,
            });

        let mut state = self.state.write().await;
        state.url = Some(probe.url);
        state.title = non_empty(probe.title);
        state.page_kind = Some(probe.page_kind);
        state.media = media;
        state.search_results = search_results;
        Ok(())
    }

    async fn monitor(&mut self) {
        let exited = match self.child.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(error) => {
                    self.set_error(anyhow!("failed to inspect Firefox: {error}"))
                        .await;
                    false
                }
            },
            None => true,
        };

        if exited {
            self.shutdown_current().await;
            self.set_error(anyhow!("Firefox is not running; restarting"))
                .await;
            if let Err(error) = self.ensure_started().await {
                self.set_error(error).await;
            }
        } else if self.bidi.is_some() && self.context.is_some() {
            if let Err(error) = self.refresh().await {
                tracing::debug!(error = %error, "Firefox state refresh failed");
            }
        }
    }

    async fn child_is_alive(&mut self) -> Result<bool> {
        match self.child.as_mut() {
            Some(child) => Ok(child
                .try_wait()
                .context("failed to inspect Firefox process")?
                .is_none()),
            None => Ok(false),
        }
    }

    async fn shutdown_current(&mut self) {
        self.bidi = None;
        self.context = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    fn context(&self) -> Result<&str> {
        self.context
            .as_deref()
            .ok_or_else(|| anyhow!("Firefox browsing context is not ready"))
    }

    fn bidi_mut(&mut self) -> Result<&mut BidiClient> {
        self.bidi
            .as_mut()
            .ok_or_else(|| anyhow!("Firefox BiDi connection is not ready"))
    }

    async fn key_press(&mut self, context: &str, keys: Vec<KeyStroke>) -> Result<()> {
        self.bidi_mut()?.key_press(context, keys).await
    }

    async fn shifted_key(&mut self, context: &str, key: &str) -> Result<()> {
        self.key_press(
            context,
            vec![
                KeyStroke {
                    value: "\u{e008}".to_string(),
                    down_up: false,
                },
                KeyStroke::press(key),
            ],
        )
        .await?;
        self.release_actions(context).await
    }

    async fn release_actions(&mut self, context: &str) -> Result<()> {
        self.bidi_mut()?.release_actions(context).await
    }

    async fn set_status(&self, status: BrowserStatus, error: Option<String>) {
        let mut state = self.state.write().await;
        state.status = status;
        state.error = error;
    }

    async fn set_error(&self, error: anyhow::Error) {
        self.set_status(BrowserStatus::Error, Some(error.to_string()))
            .await;
    }
}

fn spawn_output_reader<R>(reader: R, endpoint_tx: mpsc::Sender<String>, target: &'static str)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(endpoint) = extract_endpoint(&line) {
                let _ = endpoint_tx.send(endpoint).await;
            }
            tracing::debug!(target, "Firefox: {line}");
        }
    });
}

fn extract_endpoint(line: &str) -> Option<String> {
    let endpoint = line
        .split_whitespace()
        .map(|token| token.trim_matches(|character| matches!(character, '(' | ')' | ',' | ';')))
        .find(|token| token.starts_with("ws://127.0.0.1:") || token.starts_with("ws://localhost:"))
        .map(ToString::to_string)?;
    if endpoint.ends_with("/session") {
        Some(endpoint)
    } else {
        Some(format!("{endpoint}/session"))
    }
}

fn finite(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::extract_endpoint;

    #[test]
    fn extracts_firefox_bidi_endpoint() {
        assert_eq!(
            extract_endpoint("WebDriver BiDi listening on ws://127.0.0.1:41234/session"),
            Some("ws://127.0.0.1:41234/session".to_string())
        );
        assert_eq!(extract_endpoint("ordinary Firefox log"), None);
    }
}
