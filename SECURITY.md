# Security

CouchMote is designed for one trusted Linux TV box and one private Tailscale
network. It is not a public web service.

## Defaults

- HTTP binds to loopback and discovered Tailscale addresses only.
- Requests from non-loopback, non-Tailscale source addresses are rejected.
- The phone must complete one-time pairing before any control API works.
- Pairing codes expire after ten minutes and are rate limited per source IP.
- Remembered sessions are stored as hashes with restrictive local permissions.
- The Firefox WebDriver BiDi port is random and loopback-only.
- The service exposes no shell execution or arbitrary browser-script endpoint.
- YouTube navigation is limited to HTTPS YouTube watch URLs and CouchMote’s
  fixed YouTube home/search flows.

## Sensitive local state

The dedicated Firefox profile can contain YouTube cookies and must be treated
as private. CouchMote stores it and session hashes below the configured state
directory. Do not copy that directory into a repository or backup it into an
untrusted location.

Do not expose port `8791` through port forwarding. If a stable browser-trusted
HTTPS URL is needed, place the loopback listener behind an appropriately
restricted Tailscale Serve configuration.

## Reporting a vulnerability

Do not publish pairing codes, session files, Firefox profiles, real tailnet
hostnames, or access logs in a public issue. Contact the repository owner with
the smallest reproducible description possible.
