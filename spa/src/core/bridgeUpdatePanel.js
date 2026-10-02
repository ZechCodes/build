import { esc } from "./text.js";
import {
  bridgeCanInstall, bridgeCanReplaceDevelopmentBuild, bridgeUpdateAvailable, bridgeUpdateStatus,
  onBridgeUpdatesChanged, bridgeUpdateCacheGeneration, bridgeUpdateRevision, refreshBridgeUpdateStatus,
  rememberBridgeUpdateStatus, watchBridgeUpdateDevice,
} from "./bridgeUpdates.js";

/** What replaces a development build the app cannot replace. */
const INSTALL_COMMAND = "curl -fsSL https://getbuild.ing/install.sh | sh";

const dateText = (value) => {
  if (!value) return "Never";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unknown" : date.toLocaleString();
};

/** A failed install. A development build from before wire 3.5.0 (no
 *  can_replace_development_build) never installed, so its "failed" was a
 *  check's error that stuck. */
const installFailed = (status) => status.state === "failed"
  && (!status.development_build || "can_replace_development_build" in status);

function stateText(status) {
  if (status.state === "scheduled_when_idle") return "Queued until agents are done.";
  if (status.state === "installing") return "Installing update. The bridge will reconnect shortly.";
  if (installFailed(status)) return "The update failed.";
  if (status.last_error) return "Could not check for updates.";
  if (!status.last_checked_at && !status.latest_release) return "Not checked yet.";
  return bridgeUpdateAvailable(status)
    ? `Version ${status.latest_release?.version || "new"} is available.`
    : "Up to date.";
}

function developmentNote(status) {
  if (!status.development_build) return "";
  if (bridgeCanReplaceDevelopmentBuild(status)) {
    return '<p class="dim">This bridge is a development build. Installing a release replaces it.</p>';
  }
  return `<p class="dim">This bridge is a development build, which cannot update from the app. To replace it with a release build, run this on the machine:</p>
    <pre class="bridge-update-command"><code>${esc(INSTALL_COMMAND)}</code></pre>`;
}

function statusBody(status, loading) {
  if (!status) return `<p class="dim">${loading ? "Loading bridge version…" : "Update controls are unavailable on this bridge. Install a current bridge to update it from the app."}</p>`;
  return `<div class="row"><span class="k">Running version</span><span class="v">${esc(status.running_version)}</span></div>
    <div class="row"><span class="k">Platform</span><span class="v">${esc(status.platform || "Unknown")}</span></div>
    <div class="row"><span class="k">Latest release</span><span class="v">${esc(status.latest_release?.version || "Not checked yet")}</span></div>
    <div class="row"><span class="k">Last checked</span><span class="v">${esc(dateText(status.last_checked_at))}</span></div>
    <p class="bridge-update-state">${esc(stateText(status))}</p>
    ${developmentNote(status)}
    ${status.last_error ? `<p class="bridge-update-error" role="alert">${esc(status.last_error)}</p>` : ""}`;
}

/** The warning a development build's replacement waits behind. */
function replacementWarning(status, when) {
  const version = esc(status.latest_release?.version || "the latest release");
  const timing = when === "idle" ? "once agents are done" : "now";
  return `<div class="bridge-update-confirm" role="group" aria-label="Replace the development build">
    <p>This replaces this development build with release ${version} ${timing}. The bridge restarts, and changes from your local source are no longer in the running bridge.</p>
    <div class="bridge-update-actions">
      <button class="btn primary" data-bridge-confirm-replace type="button">Replace with ${version}</button>
      <button class="btn" data-bridge-confirm-cancel type="button">Cancel</button>
    </div>
  </div>`;
}

const installOffered = (status) => bridgeCanInstall(status) || bridgeCanReplaceDevelopmentBuild(status);

function setActionAvailability({ check, now, idle }, status, busy) {
  const installing = status?.state === "installing";
  const scheduled = status?.state === "scheduled_when_idle";
  const canInstall = installOffered(status) && !installing;
  check.disabled = busy || !status || installing;
  now.disabled = busy || !canInstall;
  idle.disabled = busy || !canInstall || scheduled;
}

const INSTALL_NOTES = { now: "Starting installation…", idle: "Scheduling installation…" };

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
    <div class="bridge-update-confirm-host"></div>
    <p class="bridge-update-request" role="status" aria-live="polite"></p>
  </div>`;
  const body = host.querySelector(".bridge-update-body");
  const check = host.querySelector("[data-bridge-check]");
  const now = host.querySelector("[data-bridge-install-now]");
  const idle = host.querySelector("[data-bridge-install-idle]");
  const request = host.querySelector(".bridge-update-request");
  const confirmHost = host.querySelector(".bridge-update-confirm-host");
  // The install a development build's replacement warning is holding: "now",
  // "idle", or null while no warning shows.
  let confirming = null;

  const paintConfirmation = (status) => {
    if (!bridgeCanReplaceDevelopmentBuild(status) || busy) confirming = null;
    confirmHost.innerHTML = confirming ? replacementWarning(status, confirming) : "";
  };

  const paint = () => {
    if (!active) return;
    const status = bridgeUpdateStatus(deviceId);
    body.innerHTML = statusBody(status, loading);
    setActionAvailability({ check, now, idle }, status, busy);
    paintConfirmation(status);
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
      const generation = await bridgeUpdateCacheGeneration(deviceId);
      if (!active) return;
      const status = await callRpc(method, args);
      if (!active) return;
      if (bridgeUpdateRevision(deviceId) === revision) {
        await rememberBridgeUpdateStatus(deviceId, status, generation);
      }
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
  const install = (when) => {
    if (bridgeCanReplaceDevelopmentBuild(bridgeUpdateStatus(deviceId))) {
      confirming = when;
      paint();
      return;
    }
    void act("bridge.install_update", { when }, INSTALL_NOTES[when]);
  };
  check.onclick = () => void act("bridge.check_update", {}, "Starting update check…");
  now.onclick = () => install("now");
  idle.onclick = () => install("idle");
  confirmHost.onclick = (event) => {
    const when = confirming;
    if (event.target.closest("[data-bridge-confirm-cancel]")) {
      confirming = null;
      paint();
    } else if (when && event.target.closest("[data-bridge-confirm-replace]")) {
      confirming = null;
      void act("bridge.install_update", { when, replace_development_build: true }, INSTALL_NOTES[when]);
    }
  };

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
