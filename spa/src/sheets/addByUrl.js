import { beginHttpDiscovery, completeHttpDiscovery, discardHttpDiscovery } from "../core/httpConnections.js";
import { httpFailure } from "../core/httpEndpoint.js";
import { settingsSheetHtml } from "./settingsSheet.js";

const defaultServices = {
  begin: beginHttpDiscovery, complete: completeHttpDiscovery, discard: discardHttpDiscovery,
  navigate(url) { const target = new URL(url); target.hash = location.hash; location.assign(target.href); },
  async connect(identity) {
    const [{ refreshDevices }, { chooseDeviceTransport }, { App }] = await Promise.all([
      import("../devices.js"), import("../connection.js"), import("../appState.js"),
    ]);
    await refreshDevices();
    if (!App.devices.some((device) => device.id === identity.deviceId)) throw httpFailure("authentication");
    await chooseDeviceTransport(identity.deviceId, { transport: "http", identity });
  },
};

export function openAddByUrl(onDone = () => {}, { challengeId, services = defaultServices } = {}) {
  const sheet = document.querySelector("#sheet");
  const scrim = document.querySelector("#scrim");
  sheet.innerHTML = settingsSheetHtml({ title: "Add by URL", subtitleHtml: "Connect to an HTTPS endpoint on a device already paired with your account.",
    bodyHtml: `<form id="urlform"><label for="bridge-url">Bridge HTTPS address</label>
      <input id="bridge-url" type="url" inputmode="url" autocomplete="off" autocapitalize="none" spellcheck="false" placeholder="https://bridge.example:9443" required />
      <p class="dim">The endpoint needs a trusted certificate and network access from this device. A browser may ask for local-network permission.</p>
      <button class="btn primary" id="urlverify" type="submit">Verify address</button></form>
      <div id="urlidentity" hidden></div><div id="urlstatus" role="status" aria-live="polite"></div>
      <div id="urlerror" class="adderr" role="alert"></div>
      <div class="row"><button class="btn" id="urlcancel" style="margin-left:auto">Cancel</button></div>`,
  });
  const form = sheet.querySelector("#urlform");
  const input = sheet.querySelector("#bridge-url");
  const status = sheet.querySelector("#urlstatus");
  const error = sheet.querySelector("#urlerror");
  const identityBox = sheet.querySelector("#urlidentity");
  let active = true;
  let identity;
  let connected = false;
  let revision = 0;
  const current = () => active && sheet.contains(form) && scrim.classList.contains("show");
  const dispose = () => {
    active = false; revision += 1;
    if (identity && !connected) services.discard(identity);
    if (sheet.contains(form)) scrim.classList.remove("show");
  };
  const reportError = (failure) => {
    status.textContent = "";
    error.textContent = httpFailure(failure?.code || "network").message;
    sheet.querySelector("#urlverify").disabled = false;
  };
  const showIdentity = (verified) => {
    identity = verified;
    form.hidden = true; identityBox.hidden = false; status.textContent = "";
    identityBox.replaceChildren();
    const name = document.createElement("p"); name.textContent = verified.name || verified.deviceId;
    const details = document.createElement("p"); details.className = "dim";
    details.style.overflowWrap = "anywhere";
    details.textContent = `${verified.origin} · Device ${verified.deviceId}${verified.fingerprint ? ` · ${verified.fingerprint}` : ""}`;
    const note = document.createElement("p"); note.className = "dim";
    note.textContent = "Use HTTPS for views and actions. The terminal uses WebRTC separately and may be unavailable. This choice is saved for this device.";
    const connect = document.createElement("button"); connect.id = "urlconnect"; connect.className = "btn primary"; connect.textContent = "Use HTTPS";
    connect.onclick = async () => {
      if (!current() || connect.disabled) return;
      connect.disabled = true; error.textContent = ""; status.textContent = "Connecting…";
      try {
        await services.connect(verified);
        connected = true; dispose(); await onDone();
      } catch (failure) { if (current()) { reportError(failure); connect.disabled = false; } }
    };
    identityBox.append(name, details, note, connect);
  };
  const verify = async (operation) => {
    const version = ++revision;
    error.textContent = ""; status.textContent = "Verifying address…";
    sheet.querySelector("#urlverify").disabled = true;
    try {
      const result = await operation();
      if (!current() || version !== revision) { if (result.identity) services.discard(result.identity); return; }
      if (result.navigateTo) services.navigate(result.navigateTo);
      else showIdentity(result.identity);
    } catch (failure) { if (current() && version === revision) reportError(failure); }
  };
  scrim.classList.add("show");
  sheet.querySelector("#urlcancel").onclick = dispose;
  form.onsubmit = (event) => { event.preventDefault(); if (current()) void verify(() => services.begin(input.value)); };
  if (challengeId) void verify(async () => ({ identity: await services.complete(challengeId) }));
  else input.focus();
  return dispose;
}

// Non-secret account-bound challenge IDs survive the CSP document reload. The
// server supplies their canonical origin; no token or origin is taken from URL.
export function resumeAddByUrl(onDone) {
  const url = new URL(location.href);
  const challengeId = url.searchParams.get("bridge_discovery");
  if (!challengeId) return;
  url.searchParams.delete("bridge_discovery");
  history.replaceState(history.state, "", url.href);
  return openAddByUrl(onDone, { challengeId });
}
