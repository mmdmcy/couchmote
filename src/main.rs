mod admin;
mod audio;
mod auth;
mod bidi;
mod browser;
mod config;
mod model;
mod network;
mod server;
mod setup;
mod youtube;

use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::{
    admin::AdminRequest,
    audio::AudioController,
    config::{Config, ListenMode},
    server::AppState,
};

#[derive(Debug, Parser)]
#[command(
    name = "couchmote",
    version,
    about = "A lightweight YouTube couch remote"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<CommandKind>,
}

#[derive(Debug, Subcommand)]
enum CommandKind {
    Serve(ServeArgs),
    /// Launch the dedicated Firefox profile for one-time sign-in/setup.
    BrowserSetup,
    /// Generate a one-time phone pairing code through the local admin socket.
    Pair,
    /// Show the current browser and TV audio state.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Check host dependencies without starting the browser.
    Doctor,
    /// Revoke every remembered phone session.
    Revoke,
}

#[derive(Debug, Args)]
struct ServeArgs {
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    listen: Option<String>,
    #[arg(long)]
    browser: Option<String>,
    #[arg(long)]
    profile_dir: Option<PathBuf>,
    /// Do not open the first-run setup page in the local desktop browser.
    #[arg(long)]
    no_setup: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("couchmote=info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.command {
        None => serve(default_serve_args()).await,
        Some(CommandKind::Serve(args)) => serve(args).await,
        Some(CommandKind::BrowserSetup) => browser_setup().await,
        Some(CommandKind::Pair) => admin_pair().await,
        Some(CommandKind::Status { json }) => admin_status(json).await,
        Some(CommandKind::Doctor) => doctor().await,
        Some(CommandKind::Revoke) => admin_revoke().await,
    }
}

fn default_serve_args() -> ServeArgs {
    ServeArgs {
        port: None,
        listen: None,
        browser: None,
        profile_dir: None,
        no_setup: false,
    }
}

async fn serve(args: ServeArgs) -> Result<()> {
    let mut config = Config::load()?;
    if let Some(port) = args.port {
        if port == 0 {
            return Err(anyhow!("--port must be between 1 and 65535"));
        }
        config.port = port;
    }
    if let Some(listen) = args.listen {
        config.listen = ListenMode::parse(&listen)?;
    }
    if let Some(browser) = args.browser {
        config.browser = browser;
    }
    if let Some(profile_dir) = args.profile_dir {
        config.profile_dir = profile_dir;
    }

    let state = AppState::build(config.clone()).await?;
    let open_setup = !args.no_setup && !config.setup_complete().await?;
    let admin_path = config.runtime_socket.clone();
    let admin_task = tokio::spawn(admin::run(admin_path.clone(), state.clone()));

    match state.auth.issue_pairing_code().await {
        Ok(pairing) => {
            println!(
                "Phone pairing code: {} (expires at {})",
                pairing.code, pairing.expires_at
            );
        }
        Err(error) => tracing::warn!(error = %error, "could not issue startup pairing code"),
    }

    let result = server::run(state, open_setup).await;
    admin_task.abort();
    admin::remove_socket(&admin_path).await;
    result
}

async fn browser_setup() -> Result<()> {
    let config = Config::load()?;
    config.ensure_directories().await?;
    Command::new(&config.browser)
        .args([
            "--no-remote",
            "--new-instance",
            "--profile",
            config.profile_dir.to_string_lossy().as_ref(),
            "--new-window",
            youtube::YOUTUBE_HOME,
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to start Firefox using {}", config.browser))?;
    println!(
        "Dedicated Firefox setup profile launched at {}.",
        config.profile_dir.display()
    );
    println!("Sign in to YouTube on the TV, then close that window and run couchmote serve.");
    Ok(())
}

async fn admin_pair() -> Result<()> {
    let config = Config::load()?;
    let response = admin::request(&config.runtime_socket, AdminRequest::Pair).await?;
    println!(
        "Phone pairing code: {} (expires at {})",
        response["code"].as_str().unwrap_or("unknown"),
        response["expires_at"].as_i64().unwrap_or_default()
    );
    Ok(())
}

async fn admin_status(json_output: bool) -> Result<()> {
    let config = Config::load()?;
    let response = admin::request(&config.runtime_socket, AdminRequest::Status).await?;
    let state = response
        .get("state")
        .cloned()
        .ok_or_else(|| anyhow!("admin response contained no state"))?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&state)?);
    } else {
        println!(
            "Browser: {}\nPage: {}\nVolume: {}%{}",
            state["browser"]["status"].as_str().unwrap_or("unknown"),
            state["browser"]["title"]
                .as_str()
                .unwrap_or("nothing playing"),
            state["volume"]["percent"].as_u64().unwrap_or(0),
            if state["volume"]["muted"].as_bool().unwrap_or(false) {
                " (muted)"
            } else {
                ""
            }
        );
    }
    Ok(())
}

async fn admin_revoke() -> Result<()> {
    let config = Config::load()?;
    admin::request(&config.runtime_socket, AdminRequest::Revoke).await?;
    println!("All remembered CouchMote phone sessions were revoked.");
    Ok(())
}

async fn doctor() -> Result<()> {
    let config = Config::load()?;
    println!("CouchMote doctor");
    print_check(
        "Firefox",
        Command::new(&config.browser)
            .arg("--version")
            .output()
            .is_ok(),
    );
    print_check("DISPLAY", std::env::var_os("DISPLAY").is_some());
    print_check("pactl", AudioController::new().check().await.is_ok());
    print_check(
        "Tailscale address",
        !network::tailnet_addresses()?.is_empty(),
    );
    print_check(
        "profile directory parent",
        config.profile_dir.parent().is_some(),
    );
    println!("HTTP mode: {:?}", config.listen);
    println!("HTTP port: {}", config.port);
    println!("Profile: {}", config.profile_dir.display());
    println!("Admin socket: {}", config.runtime_socket.display());
    Ok(())
}

fn print_check(name: &str, ok: bool) {
    println!("  [{}] {name}", if ok { "ok" } else { "!!" });
}
