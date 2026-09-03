#!/usr/bin/env bash
set -euo pipefail

script_path="$(readlink -f "${BASH_SOURCE[0]}")"
repo_dir="$(dirname "$(dirname "$script_path")")"
user_home="${HOME:?HOME is not set}"
bin_dir="$user_home/.local/bin"
app_dir="$user_home/.local/share/applications"

if [[ "${1:-}" != "--in-terminal" && ! -t 1 ]]; then
  if command -v x-terminal-emulator >/dev/null 2>&1; then
    exec x-terminal-emulator -e "$script_path" --in-terminal
  fi
fi

fail() {
  if command -v zenity >/dev/null 2>&1 && [[ -n "${DISPLAY:-}" ]]; then
    zenity --error --title="CouchMote installer" --text="$1" || true
  fi
  printf 'CouchMote installer: %s\n' "$1" >&2
  exit 1
}

command -v cargo >/dev/null 2>&1 || fail "Rust/Cargo is not installed. Install Rust, then double-click this installer again."

cd "$repo_dir"
printf 'Building CouchMote…\n'
cargo build --release || fail "The build failed. The error above explains what needs attention."

install -Dm755 target/release/couchmote "$bin_dir/couchmote" \
  || fail "Could not install the CouchMote application."
install -Dm644 packaging/desktop/couchmote.desktop "$app_dir/couchmote.desktop" \
  || fail "Could not install the Applications menu entry."

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$app_dir" >/dev/null 2>&1 || true
fi

printf '\nCouchMote is installed. Open it from the Applications menu.\n'
if command -v zenity >/dev/null 2>&1 && [[ -n "${DISPLAY:-}" ]]; then
  if zenity --question --title="CouchMote is ready" --text="CouchMote is installed. Start it now?"; then
    "$bin_dir/couchmote" >/dev/null 2>&1 &
  fi
else
  printf 'Run: %s/couchmote\n' "$bin_dir"
fi
