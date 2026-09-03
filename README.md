# CouchMote

CouchMote is a small, open-source YouTube remote for a Linux TV box. The TV
keeps showing YouTube in a dedicated Firefox window while a phone controls it
from a large, touch-friendly web remote.

It is intentionally narrower than a remote desktop tool:

- no screen streaming;
- no mouse precision or remote zooming;
- no Node/Electron runtime;
- no arbitrary shell, URL, or JavaScript control;
- no YouTube API key or downloader.

The current target is Linux Mint/Cinnamon on X11 with Firefox, Tailscale, and
PipeWire/PulseAudio compatibility through `pactl`.

## Click-through setup (Linux Mint)

For the normal TV setup, you should not need to use the terminal:

1. Double-click [`scripts/install.sh`](scripts/install.sh) in the file manager and choose **Run in Terminal** if Mint asks.
2. Open **CouchMote** from the Applications menu.
3. Follow the setup window on the TV: let it check the box, use the displayed pairing code on the iPhone, and click **Finish setup**.

The setup window creates the dedicated Firefox profile, shows the Tailscale
address for the phone, and can start CouchMote automatically at login. It is
local-only and never exposes host setup actions to the iPhone.

## Manual/source setup

Install the host dependencies:

```sh
sudo apt install firefox pulseaudio-utils
```

Build CouchMote from source:

```sh
cargo build --release
install -Dm755 target/release/couchmote ~/.local/bin/couchmote
```

The host must run CouchMote as the same desktop user that owns the visible X11
session. Confirm the environment first:

```sh
couchmote doctor
```

The first launch automatically creates and opens the separate persistent
Firefox profile. If you want subscriptions and recommendations, sign in to
YouTube once in that TV window; the profile will remember it.

`couchmote browser-setup` remains available as an advanced manual helper when
the service is stopped.

Start the remote service:

```sh
couchmote serve
```

On a first run, `couchmote serve` also opens the local setup page. The default
listener is loopback plus any local Tailscale addresses on port `8791`. Open
the displayed URL on your iPhone while Tailscale is connected, enter the code,
and use the remote.

Generate another pairing code later with:

```sh
couchmote pair
```

Remembered phone sessions last 30 days. Revoke all of them with:

```sh
couchmote revoke
```

## Optional systemd user service

The setup wizard's **Start CouchMote automatically** option is the easiest
choice. For users who already manage graphical services with systemd, copy
[`packaging/systemd/couchmote.service`](packaging/systemd/couchmote.service)
to `~/.config/systemd/user/`, adjust the binary path if needed, and import the
graphical session variables:

```sh
mkdir -p ~/.config/systemd/user
cp packaging/systemd/couchmote.service ~/.config/systemd/user/
systemctl --user import-environment DISPLAY XAUTHORITY XDG_RUNTIME_DIR
systemctl --user daemon-reload
systemctl --user enable --now couchmote.service
journalctl --user -u couchmote.service -f
```

If the TV box uses a different X display or Xauthority path, edit the unit or
export `DISPLAY` and `XAUTHORITY` before starting it. CouchMote does not need
root privileges.

## Configuration

Environment variables are documented in [`.env.example`](.env.example):

| Variable | Default | Purpose |
| --- | --- | --- |
| `COUCHMOTE_LISTEN` | `tailnet` | `tailnet` or `loopback` |
| `COUCHMOTE_PORT` | `8791` | HTTP port |
| `COUCHMOTE_BROWSER` | `firefox` | Firefox executable or PATH name |
| `COUCHMOTE_STATE_DIR` | `$XDG_STATE_HOME/couchmote` | Runtime state and sessions |
| `COUCHMOTE_PROFILE_DIR` | `<state>/firefox-profile` | Dedicated Firefox profile |
| `COUCHMOTE_SOCKET` | `$XDG_RUNTIME_DIR/couchmote.sock` | Local admin socket |

Command-line flags on `serve` override the matching environment values. Use
`couchmote serve --no-setup` for a service that must never open a local setup
window.

## Controls

The phone remote provides YouTube search and result selection, direct YouTube
watch URLs, play/pause, ten-second seek, previous/next, fullscreen, back/home,
directional page navigation, and TV volume/mute.

The phone controls the browser through a local Firefox WebDriver BiDi
connection. CouchMote only exposes fixed operations to the phone; it never
forwards the browser debugging port over Tailscale.

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
./scripts/smoke.sh
```

Live YouTube acceptance requires a graphical Firefox session and an active
network connection. Automated tests use fixtures and a fake BiDi server so
they do not depend on YouTube layout or availability.

## Relationship to other projects

`rustopviewer` remains the general browser-based desktop remote. CouchMote is a
separate, smaller media-control workflow for a TV box and does not depend on
RustOpViewer, LinuxMice, Homefleet, or Masterdale.

## License

Licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)
