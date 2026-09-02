# Contributing to CouchMote

CouchMote is intentionally small and Linux-first. Keep the control surface
focused on safe, comfortable media use from a phone.

Before opening a change, run:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Do not commit Firefox profiles, pairing/session state, `.env` files, YouTube
cookies, or host-specific systemd overrides. Changes that add a new command,
HTTP endpoint, browser capability, or network exposure should include a short
security note and tests.
