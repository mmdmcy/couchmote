use anyhow::{Context, Result, anyhow};
use std::process::Output;
use tokio::process::Command;

use crate::model::VolumeState;

#[derive(Debug, Clone)]
pub struct AudioController {
    command: String,
}

impl Default for AudioController {
    fn default() -> Self {
        Self {
            command: "pactl".to_string(),
        }
    }
}

impl AudioController {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn status(&self) -> VolumeState {
        let volume = self.run(["get-sink-volume", "@DEFAULT_SINK@"]).await;
        let mute = self.run(["get-sink-mute", "@DEFAULT_SINK@"]).await;

        match (volume, mute) {
            (Ok(volume), Ok(mute)) => match (parse_percent(&volume), parse_mute(&mute)) {
                (Some(percent), Some(muted)) => VolumeState {
                    available: true,
                    percent,
                    muted,
                    error: None,
                },
                _ => unavailable("pactl returned an unrecognized sink status"),
            },
            (Err(volume), _) => unavailable(volume.to_string()),
            (_, Err(mute)) => unavailable(mute.to_string()),
        }
    }

    pub async fn set_volume(&self, percent: u8) -> Result<()> {
        let percent = percent.min(100);
        self.run(["set-sink-volume", "@DEFAULT_SINK@", &format!("{percent}%")])
            .await
            .map(|_| ())
    }

    pub async fn toggle_mute(&self) -> Result<()> {
        self.run(["set-sink-mute", "@DEFAULT_SINK@", "toggle"])
            .await
            .map(|_| ())
    }

    pub async fn check(&self) -> Result<String> {
        let output = self
            .run(["--version"])
            .await
            .context("pactl is unavailable")?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn run<'a, I>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let output = Command::new(&self.command)
            .args(args)
            .output()
            .await
            .with_context(|| format!("failed to execute {}", self.command))?;
        if !output.status.success() {
            return Err(anyhow!(
                "{} failed: {}",
                self.command,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(output)
    }
}

fn parse_percent(output: &Output) -> Option<u8> {
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .find_map(|token| token.strip_suffix('%')?.parse::<u8>().ok())
}

fn parse_mute(output: &Output) -> Option<bool> {
    let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    if text.contains("mute: yes") {
        Some(true)
    } else if text.contains("mute: no") {
        Some(false)
    } else {
        None
    }
}

fn unavailable(error: impl Into<String>) -> VolumeState {
    VolumeState {
        available: false,
        percent: 0,
        muted: false,
        error: Some(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_mute, parse_percent};
    use std::process::{ExitStatus, Output};

    #[cfg(unix)]
    fn success() -> ExitStatus {
        std::os::unix::process::ExitStatusExt::from_raw(0)
    }

    #[test]
    fn parses_pactl_volume() {
        let output = Output {
            status: success(),
            stdout:
                b"Volume: front-left: 65536 / 72% / 0.00 dB, front-right: 65536 / 72% / 0.00 dB\n"
                    .to_vec(),
            stderr: Vec::new(),
        };
        assert_eq!(parse_percent(&output), Some(72));
    }

    #[test]
    fn parses_pactl_mute() {
        let yes = Output {
            status: success(),
            stdout: b"Mute: yes\n".to_vec(),
            stderr: Vec::new(),
        };
        let no = Output {
            status: success(),
            stdout: b"Mute: no\n".to_vec(),
            stderr: Vec::new(),
        };
        assert_eq!(parse_mute(&yes), Some(true));
        assert_eq!(parse_mute(&no), Some(false));
    }
}
