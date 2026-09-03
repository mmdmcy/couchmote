(() => {
  "use strict";

  const connection = document.getElementById("setup-connection");
  const checks = document.getElementById("setup-checks");
  const browserStatus = document.getElementById("browser-status");
  const pairCode = document.getElementById("pair-code");
  const pairExpiry = document.getElementById("pair-expiry");
  const phoneLinks = document.getElementById("phone-links");
  const finishButton = document.getElementById("finish-button");
  const finishMessage = document.getElementById("finish-message");
  const doneCard = document.getElementById("done-card");
  const autostart = document.getElementById("autostart");
  let latestStatus = null;

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

  function message(node, text, error = false) {
    node.textContent = text || "";
    node.classList.toggle("error", error);
  }

  function setConnection(online, error = false) {
    connection.dataset.state = error ? "error" : online ? "online" : "offline";
    connection.textContent = error ? "Needs attention" : online ? "Connected" : "Offline";
  }

  function renderChecks(items) {
    checks.replaceChildren();
    for (const item of items || []) {
      const row = document.createElement("div");
      row.className = "setup-check";
      row.dataset.ok = item.ok ? "true" : "false";

      const icon = document.createElement("span");
      icon.className = "setup-check-icon";
      icon.textContent = item.ok ? "✓" : "!";

      const text = document.createElement("div");
      const title = document.createElement("strong");
      title.textContent = item.label || item.id;
      const detail = document.createElement("small");
      detail.textContent = item.detail || "";
      text.append(title, detail);
      row.append(icon, text);
      checks.append(row);
    }
  }

  function renderPhoneLinks(urls) {
    phoneLinks.replaceChildren();
    if (!urls || !urls.length) {
      const empty = document.createElement("p");
      empty.className = "muted small";
      empty.textContent = "A Tailscale address will appear when Tailscale is connected.";
      phoneLinks.append(empty);
      return;
    }
    const label = document.createElement("p");
    label.className = "muted small";
    label.textContent = "Open on the iPhone:";
    phoneLinks.append(label);
    for (const url of urls) {
      const link = document.createElement("a");
      link.className = "phone-url";
      link.href = url;
      link.target = "_blank";
      link.rel = "noreferrer";
      link.textContent = url;
      phoneLinks.append(link);
    }
  }

  function renderStatus(status) {
    latestStatus = status;
    renderChecks(status.checks);
    renderPhoneLinks(status.tailnet_urls);
    browserStatus.textContent = status.browser_error
      ? `Firefox: ${status.browser_error}`
      : `Firefox: ${status.browser_status || "starting"}`;
    autostart.checked = status.autostart;
    finishButton.disabled = !status.can_finish;
    setConnection(true);
  }

  async function refresh() {
    try {
      renderStatus(await request("/api/setup/status", { method: "GET", headers: {} }));
    } catch (error) {
      setConnection(false, true);
      message(finishMessage, error.message, true);
    }
  }

  async function generatePairingCode() {
    pairCode.textContent = "--------";
    message(pairExpiry, "Generating a fresh code…");
    try {
      const pairing = await request("/api/setup/pair", {
        method: "POST",
        body: "{}",
      });
      pairCode.textContent = pairing.code || "--------";
      renderPhoneLinks(pairing.tailnet_urls || latestStatus?.tailnet_urls || []);
      const expiry = pairing.expires_at ? new Date(pairing.expires_at * 1000) : null;
      message(pairExpiry, expiry ? `Valid until ${expiry.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}.` : "Code ready.");
    } catch (error) {
      message(pairExpiry, error.message, true);
    }
  }

  document.getElementById("refresh-button").addEventListener("click", refresh);
  document.getElementById("new-code-button").addEventListener("click", generatePairingCode);
  document.getElementById("copy-code-button").addEventListener("click", async () => {
    const code = pairCode.textContent.trim();
    if (!/^\d{8}$/.test(code)) return;
    try {
      await navigator.clipboard.writeText(code);
      message(pairExpiry, "Pairing code copied.");
    } catch (_) {
      message(pairExpiry, "The code is shown above; select it to copy.");
    }
  });

  finishButton.addEventListener("click", async () => {
    finishButton.disabled = true;
    message(finishMessage, "Saving setup…");
    try {
      await request("/api/setup/finish", {
        method: "POST",
        body: JSON.stringify({ autostart: autostart.checked }),
      });
      doneCard.hidden = false;
      message(finishMessage, autostart.checked ? "Saved. CouchMote will start automatically." : "Saved. CouchMote is ready to use.");
      await refresh();
    } catch (error) {
      finishButton.disabled = false;
      message(finishMessage, error.message, true);
    }
  });

  refresh();
  generatePairingCode();
  setInterval(refresh, 2000);
})();
