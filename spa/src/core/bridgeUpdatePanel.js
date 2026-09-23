import { esc } from "./text.js";
import {
  bridgeCanInstall, bridgeUpdateAvailable, bridgeUpdateStatus, onBridgeUpdatesChanged,
  bridgeUpdateRevision, refreshBridgeUpdateStatus, rememberBridgeUpdateStatus, watchBridgeUpdateDevice,
} from "./bridgeUpdates.js";

const dateText = (value) => {
  if (!value) return "Never";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unknown" : date.toLocaleString();
};

function stateText(status) {
  if (status.state === "scheduled_when_idle") return "Queued until agents are done.";
  if (status.state === "installing") return "Installing update. The bridge will reconnect shortly.";
  if (status.state === "failed") return "The update failed.";
  if (!status.last_checked_at && !status.latest_release) return "Not checked yet.";
  return bridgeUpdateAvailable(status)
    ? `Version ${status.latest_release?.version || "new"} is available.`
    : "Up to date.";
}

function statusBody(status, loading) {
  if (!status) return `<p class="dim">${loading ? "Loading bridge version…" : "Update controls are unavailable on this bridge. Install a current bridge to update it from the app."}</p>`;
  return `<div class="row"><span class="k">Running version</span><span class="v">${esc(status.running_version)}</span></div>
    <div class="row"><span class="k">Platform</span><span class="v">${esc(status.platform || "Unknown")}</span></div>
    <div class="row"><span class="k">Latest release</span><span class="v">${esc(status.latest_release?.version || "Not checked yet")}</span></div>
    <div class="row"><span class="k">Last checked</span><span class="v">${esc(dateText(status.last_checked_at))}</span></div>
    <p class="bridge-update-state">${esc(stateText(status))}</p>
    ${status.development_build ? '<p class="dim">This is a development build. Check for releases here; install a release build manually to replace it.</p>' : ""}
    ${status.last_error ? `<p class="bridge-update-error" role="alert">${esc(status.last_error)}</p>` : ""}`;
}

function setActionAvailability({ check, now, idle }, status, busy) {
  const installing = status?.state === "installing";
  const scheduled = status?.state === "scheduled_when_idle";
  const canInstall = bridgeCanInstall(status) && !installing;
  check.disabled = busy || !status || installing;
  now.disabled = busy || !canInstall;
  idle.disabled = busy || !canInstall || scheduled;
}

export function mountBridgeUpdatePanel(host, { deviceId, callRpc }) {
  watchBridgeUpdateDevice(deviceId);
  let active = true;
  let loading = true;
  let busy = false;
  let requestError = "";
  let requestNote = "";
  host.innerHTML = `<div class="panel bridge-update-panel">
    <h3>Bridge updates</h3>
    <div class="bridge-update-body" role="status" aria-live="polite"></div>
    <div class="bridge-update-actions">
      <button class="btn" data-bridge-check type="button">Check for updates</button>
      <button class="btn primary" data-bridge-install-now type="button">Install now</button>
      <button class="btn" data-bridge-install-idle type="button">Install when agents are done</button>
    </div>
    <p class="bridge-update-request" role="status" aria-live="polite"></p>
  </div>`;
  const body = host.querySelector(".bridge-update-body");
  const check = host.querySelector("[data-bridge-check]");
  const now = host.querySelector("[data-bridge-install-now]");
  const idle = host.querySelector("[data-bridge-install-idle]");
  const request = host.querySelector(".bridge-update-request");

  const paint = () => {
    if (!active) return;
    const status = bridgeUpdateStatus(deviceId);
    body.innerHTML = statusBody(status, loading);
    setActionAvailability({ check, now, idle }, status, busy);
    request.textContent = requestError || requestNote;
    request.classList.toggle("bridge-update-error", Boolean(requestError));
  };

  const stop = onBridgeUpdatesChanged((changedId, status) => {
    if (changedId === deviceId) {
      if (status) {
        loading = false;
        if (!busy) requestError = "";
      }
      paint();
    }
  });

  const act = async (method, args, note) => {
    if (busy || !active) return;
    busy = true;
    requestError = "";
    requestNote = note;
    paint();
    const revision = bridgeUpdateRevision(deviceId);
    try {
      const status = await callRpc(method, args);
      if (!active) return;
      if (bridgeUpdateRevision(deviceId) === revision) await rememberBridgeUpdateStatus(deviceId, status);
      requestNote = method === "bridge.check_update" ? "Check started. Results will appear here." : "Update request accepted.";
    } catch (error) {
      if (!active) return;
      requestNote = "";
      requestError = error.message;
    } finally {
      busy = false;
      paint();
    }
  };
  check.onclick = () => void act("bridge.check_update", {}, "Starting update check…");
  now.onclick = () => void act("bridge.install_update", { when: "now" }, "Starting installation…");
  idle.onclick = () => void act("bridge.install_update", { when: "idle" }, "Scheduling installation…");

  paint();
  void refreshBridgeUpdateStatus(deviceId, callRpc, () => active).then(() => {
    loading = false;
    paint();
  }).catch((error) => {
    if (!active) return;
    loading = false;
    requestError = error.message;
    paint();
  });
  return () => { active = false; stop(); };
}
