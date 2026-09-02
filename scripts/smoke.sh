#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test

if command -v couchmote >/dev/null 2>&1; then
  couchmote doctor
else
  cargo run --quiet -- doctor
fi
