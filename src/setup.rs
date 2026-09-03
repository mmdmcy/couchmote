use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use std::{env, net::IpAddr, path::PathBuf, process::Command};

use crate::{
    audio::AudioController,
    config::Config,
    model::{BrowserSnapshot, BrowserStatus},
    network,
};

#[derive(Debug, Clone, Serialize)]
pub struct SetupCheck {
    pub id: &'static str,
    pub label: &'static str,
    pub ok: bool,
    pub required: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupStatus {
    pub checks: Vec<SetupCheck>,
    pub browser_status: BrowserStatus,
    pub browser_error: Option<String>,
    pub local_url: String,
    pub tailnet_urls: Vec<String>,
    pub autostart: bool,
    pub complete: bool,
    pub can_finish: bool,
}

pub async fn status(config: &Config, browser: &BrowserSnapshot) -> Result<SetupStatus> {
    let firefox_ok = command_succeeds(&config.browser, "--version");
    let display = env::var_os("DISPLAY")
        .map(|value| !value.is_empty())
        .unwrap_or(false);
    let audio = AudioController::new().check().await;
    let tailnet_addresses = network::tailnet_addresses().unwrap_or_default();
    let profile_ok = config.profile_dir.is_dir();
    let complete = config.setup_complete().await?;

    let checks = vec![
        SetupCheck {
            id: "firefox",
            label: "Firefox",
            ok: firefox_ok,
            required: true,
            detail: if firefox_ok {
                format!("{} is ready", config.browser)
            } else {
                format!("{} was not found; install Firefox first", config.browser)
            },
        },
        SetupCheck {
            id: "display",
            label: "TV display",
            ok: display,
            required: true,
            detail: if display {
                format!("using {}", env::var("DISPLAY").unwrap_or_default())
            } else {
                "no graphical X11 display was detected".to_string()
            },
        },
        SetupCheck {
            id: "tailscale",
            label: "Tailscale",
            ok: !tailnet_addresses.is_empty(),
            required: true,
            detail: if tailnet_addresses.is_empty() {
                "no Tailscale address found; connect Tailscale on this Mac mini".to_string()
            } else {
                format!("{} private address(es) found", tailnet_addresses.len())
            },
        },
        SetupCheck {
            id: "audio",
            label: "TV audio",
            ok: audio.is_ok(),
            required: false,
            detail: match audio {
                Ok(version) => format!("volume control is available ({version})"),
                Err(error) => format!("volume control unavailable: {error}"),
            },
        },
        SetupCheck {
            id: "profile",
            label: "Firefox profile",
            ok: profile_ok,
            required: true,
            detail: if profile_ok {
                "dedicated CouchMote profile is ready".to_string()
            } else {
                format!(
                    "profile directory is not ready: {}",
                    config.profile_dir.display()
                )
            },
        },
    ];

    let can_finish = checks
        .iter()
        .filter(|check| check.required)
        .all(|check| check.ok)
        && matches!(&browser.status, BrowserStatus::Ready);

    Ok(SetupStatus {
        checks,
        browser_status: browser.status.clone(),
        browser_error: browser.error.clone(),
        local_url: format!("http://127.0.0.1:{}", config.port),
        tailnet_urls: tailnet_addresses
            .into_iter()
            .map(|address| url_for(address, config.port))
            .collect(),
        autostart: config.autostart_path().exists(),
        complete,
        can_finish,
    })
}

pub async fn set_autostart(config: &Config, enabled: bool) -> Result<PathBuf> {
    let path = config.autostart_path();
    if !enabled {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(path),
            Err(error) => {
                return Err(error).with_context(|| format!("failed to remove {}", path.display()));
            }
        }
    }

    let executable = env::current_exe().context("could not find the CouchMote executable")?;
    let executable = executable
        .canonicalize()
        .unwrap_or(executable)
        .to_string_lossy()
        .into_owned();
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("autostart path has no parent directory"))?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("failed to create {}", parent.display()))?;

    let entry = format!(
        "[Desktop Entry]\n\
Type=Application\n\
Name=CouchMote\n\
Comment=Start the CouchMote TV remote\n\
Exec={}\n\
TryExec={}\n\
Terminal=false\n\
StartupNotify=false\n\
X-GNOME-Autostart-enabled=true\n",
        desktop_quote(&executable),
        desktop_quote(&executable),
    );
    let temporary = path.with_extension("desktop.tmp");
    tokio::fs::write(&temporary, entry)
        .await
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    tokio::fs::rename(&temporary, &path)
        .await
        .with_context(|| format!("failed to install {}", path.display()))?;
    Ok(path)
}

fn command_succeeds(command: &str, argument: &str) -> bool {
    Command::new(command)
        .arg(argument)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn url_for(address: IpAddr, port: u16) -> String {
    match address {
        IpAddr::V4(address) => format!("http://{address}:{port}"),
        IpAddr::V6(address) => format!("http://[{address}]:{port}"),
    }
}

fn desktop_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::{desktop_quote, url_for};
    use std::net::IpAddr;

    #[test]
    fn quotes_desktop_exec_paths() {
        assert_eq!(
            desktop_quote("/tmp/Couch Mote\\bin"),
            "\"/tmp/Couch Mote\\\\bin\""
        );
    }

    #[test]
    fn formats_ipv4_and_ipv6_urls() {
        let ipv4: IpAddr = "100.64.0.10".parse().expect("IPv4 should parse");
        let ipv6: IpAddr = "fd7a:115c:a1e0::1".parse().expect("IPv6 should parse");
        assert_eq!(url_for(ipv4, 8791), "http://100.64.0.10:8791");
        assert_eq!(url_for(ipv6, 8791), "http://[fd7a:115c:a1e0::1]:8791");
    }
}
