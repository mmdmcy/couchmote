(() => {
  "use strict";

  const pairScreen = document.getElementById("pair-screen");
  const remoteScreen = document.getElementById("remote-screen");
  const connection = document.getElementById("connection");
  const pairForm = document.getElementById("pair-form");
  const pairMessage = document.getElementById("pair-message");
  const remoteMessage = document.getElementById("remote-message");
  const results = document.getElementById("results");
  const volumeSlider = document.getElementById("volume-slider");
  let volumeTimer = null;

  async function request(path, options = {}) {
    const response = await fetch(path, {
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", ...(options.headers || {}) },
      ...options,
    });
    const payload = await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(payload.error || `Request failed (${response.status})`);
      error.status = response.status;
      throw error;
    }
    return payload;
  }

  function setMessage(node, message, error = false) {
    node.textContent = message || "";
    node.classList.toggle("error", error);
  }

  function setConnected(online, error = false) {
    connection.dataset.state = error ? "error" : online ? "online" : "offline";
    connection.textContent = error ? "Needs attention" : online ? "Connected" : "Offline";
  }

  function formatTime(seconds) {
    const value = Math.max(0, Math.floor(Number(seconds) || 0));
    const minutes = Math.floor(value / 60);
    const remaining = String(value % 60).padStart(2, "0");
    return `${minutes}:${remaining}`;
  }

  function renderState(state) {
    const browser = state.browser || {};
    const media = browser.media || {};
    const volume = state.volume || {};
    const title = media.title || browser.title || "Nothing playing";
    document.getElementById("media-title").textContent = title;
    document.getElementById("page-status").textContent = browser.error || `${browser.status || "unknown"} · ${browser.page_kind || "waiting"}`;
    document.getElementById("current-time").textContent = formatTime(media.position_seconds);
    document.getElementById("duration-time").textContent = formatTime(media.duration_seconds);
    const duration = Number(media.duration_seconds) || 0;
    const position = Number(media.position_seconds) || 0;
    document.getElementById("progress-bar").style.width = duration > 0 ? `${Math.min(100, (position / duration) * 100)}%` : "0%";
    const playButton = document.getElementById("play-button");
    playButton.firstChild.textContent = media.playing ? "Ⅱ" : "▶";
    playButton.querySelector("small").textContent = media.playing ? "Pause" : "Play";
    const volumeLabel = document.getElementById("volume-label");
    volumeLabel.textContent = volume.available ? `${volume.percent}%${volume.muted ? " · muted" : ""}` : "Unavailable";
    volumeSlider.disabled = !volume.available;
    if (volume.available && document.activeElement !== volumeSlider) volumeSlider.value = String(volume.percent);
    document.getElementById("mute-button").textContent = volume.muted ? "Unmute" : "Mute";
    renderResults(browser.search_results || []);
    setConnected(browser.status === "ready", browser.status === "error");
  }

  function renderResults(items) {
    results.replaceChildren();
    if (!items.length) return;
    for (const item of items) {
      const row = document.createElement("article");
      row.className = "result";
      const text = document.createElement("div");
      const title = document.createElement("p");
      title.className = "result-title";
      title.textContent = item.title || "Untitled video";
      const meta = document.createElement("p");
      meta.className = "result-meta";
      meta.textContent = [item.channel, item.duration].filter(Boolean).join(" · ");
      text.append(title, meta);
      const button = document.createElement("button");
      button.className = "primary";
      button.type = "button";
      button.textContent = "Play";
      button.addEventListener("click", () => runAction("/api/search/open", { id: item.id }));
      row.append(text, button);
      results.append(row);
    }
  }

  async function refresh() {
    try {
      const state = await request("/api/state", { method: "GET", headers: {} });
      pairScreen.hidden = true;
      remoteScreen.hidden = false;
      renderState(state);
    } catch (error) {
      if (error.status === 401) {
        pairScreen.hidden = false;
        remoteScreen.hidden = true;
        setConnected(false);
      } else {
        setConnected(false, true);
        setMessage(remoteMessage, error.message, true);
      }
    }
  }

  async function runAction(path, body) {
    try {
      const state = await request(path, { method: "POST", body: JSON.stringify(body) });
      renderState(state);
      setMessage(remoteMessage, "");
    } catch (error) {
      setMessage(remoteMessage, error.message, true);
      if (error.status === 401) await refresh();
    }
  }

  pairForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    setMessage(pairMessage, "Pairing…");
    try {
      await request("/api/pair", {
        method: "POST",
        body: JSON.stringify({ code: new FormData(pairForm).get("code") }),
      });
      pairForm.reset();
      setMessage(pairMessage, "Paired. Loading remote…");
      await refresh();
    } catch (error) {
      setMessage(pairMessage, error.message, true);
    }
  });

  document.getElementById("search-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const query = document.getElementById("search-query").value.trim();
    await runAction("/api/search", { query });
  });

  document.getElementById("url-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const url = document.getElementById("youtube-url").value.trim();
    await runAction("/api/youtube/open", { url });
  });

  document.querySelectorAll("[data-action]").forEach((button) => {
    button.addEventListener("click", () => {
      const action = button.dataset.action;
      const type = action === "seek_back" || action === "seek_forward" ? "seek" : action;
      const body = type === "seek" ? { type, seconds: action === "seek_back" ? -10 : 10 } : { type };
      runAction("/api/action", body);
    });
  });

  document.querySelectorAll("[data-nav]").forEach((button) => {
    button.addEventListener("click", () => runAction("/api/action", { type: "navigate", direction: button.dataset.nav }));
  });

  document.getElementById("wake-button").addEventListener("click", () => runAction("/api/action", { type: "launch" }));
  document.getElementById("mute-button").addEventListener("click", () => runAction("/api/action", { type: "toggle_mute" }));
  volumeSlider.addEventListener("input", () => {
    clearTimeout(volumeTimer);
    volumeTimer = setTimeout(() => runAction("/api/action", { type: "set_volume", percent: Number(volumeSlider.value) }), 120);
  });
  document.getElementById("logout-button").addEventListener("click", async () => {
    try { await request("/api/session/logout", { method: "POST", body: "{}" }); } finally { await refresh(); }
  });

  refresh();
  setInterval(refresh, 1800);
})();
