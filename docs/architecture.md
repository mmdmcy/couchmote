# Architecture

```text
iPhone Safari
    │  authenticated HTTP over Tailscale
    ▼
CouchMote HTTP server ── local Unix admin socket
    │
    ├── Browser manager ── WebDriver BiDi over 127.0.0.1
    │       └── dedicated Firefox profile in kiosk mode ── HDMI TV
    │
    └── Audio adapter ── pactl/PipeWire default HDMI sink
```

The browser manager is an actor. It owns the Firefox child process, the BiDi
WebSocket, the active browsing context, and all page automation. HTTP handlers
send typed commands to that actor rather than opening independent browser
connections. This prevents concurrent remote taps from corrupting navigation
or key-action state.

The YouTube adapter contains fixed page scripts for status, search-result
extraction, play/pause, seeking, next/previous, and fullscreen. User-provided
values are used only as validated search text or canonical YouTube video IDs.

The phone UI is static embedded HTML, CSS, and vanilla JavaScript. It polls a
small state envelope roughly twice per second; it does not stream frames or
audio. A browser restart therefore does not require reconnecting a video
stream.

The server binds loopback and discovered Tailscale interface addresses. A
network guard checks every request source. Pairing and session state are local
to CouchMote and are independent of Tailscale identity or any other fleet
service.
