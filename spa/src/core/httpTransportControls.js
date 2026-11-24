import { cachedTerminalTransport, deviceTransportFor, httpPreferenceFor, onHttpPreferencesChanged, restoreHttpConnections } from "./httpConnections.js";
import { chooseDeviceTransport, connectDevice } from "../connection.js";
import { App } from "../appState.js";
import { onDevicesChanged } from "../devices.js";
import { terminalCompatibilityText, transportReasonText } from "./deviceTransportSelection.js";
import { contextFor, onDeviceStateChanged } from "./deviceContexts.js";
import { httpFailure } from "./httpEndpoint.js";
import { httpNetworkPermission, requestHttpNetworkPermission } from "./httpNetworkPermission.js";

const transportProblems = new Map([
  ["http-sign-in", "sign_in"],
  ["http-wrong-bridge", "wrong_bridge"],
  ["http-permission", "network_permission"],
  ["http-permission-denied", "network_permission_denied"],
  ["http-rate-limited", "rate_limited"],
  ["http-network", "network"],
  ["http-auth", "authentication"],
  ["http-unsupported", "unsupported"],
]);
const showPermission = (problem, origin) => problem === "network_permission"
  || (problem === "network" && httpNetworkPermission(origin));

// This is saved configuration. Reachability never controls the surrounding
// settings rendering; a failed HTTPS choice remains explicit and reversible.
export function mountHttpTransportControls(host, deviceId) {
  host.innerHTML = `<h3>Connection transport</h3>
    <label for="device-transport">Views and actions</label>
    <select id="device-transport"><option value="webrtc">WebRTC</option><option value="http">HTTPS and live updates</option></select>
    <form data-http-address><label for="device-bridge-url">Bridge HTTPS address</label>
      <div class="projfolder"><input id="device-bridge-url" type="url" inputmode="url" autocomplete="off" autocapitalize="none" spellcheck="false" required>
      <button class="btn" type="submit">Save and use HTTPS</button></div></form>
    <p class="dim" data-http-manual>Terminals also use HTTPS when supported. Older setups may use WebRTC. Transport changes do not resend uncertain actions.</p>
    <button class="btn" type="button" data-http-remove hidden>Remove HTTPS address</button>
    <p data-http-policy role="status" hidden></p>
    <p role="status" aria-live="polite" data-http-status></p>
    <button class="btn" type="button" data-http-permission hidden>Allow browser network access</button>`;
  const select = host.querySelector("select");
  const form = host.querySelector("form");
  const address = form.querySelector("input");
  const remove = host.querySelector("[data-http-remove]");
  const permission = host.querySelector("[data-http-permission]");
  const status = host.querySelector("[data-http-status]");
  const policyStatus = host.querySelector("[data-http-policy]");
  const manualControls = [select, host.querySelector('label[for="device-transport"]'), form, host.querySelector("[data-http-manual]")];
  let active = true;
  let shownOrigin;
  const paintStatus = () => {
    if (!active || !policyStatus.hidden) return;
    paintLegacyStatus();
  };
  const paintLegacyStatus = () => {
    const context = contextFor(deviceId);
    const chosen = httpPreferenceFor(deviceId);
    if (chosen?.selected !== "http") { permission.hidden = true; return; }
    const problem = transportProblems.get(context?.blocked);
    if (!context?.session && !context?.blocked) return;
    status.textContent = problem ? httpFailure(problem).message : "";
    permission.hidden = !showPermission(problem, chosen.origin);
  };
  const paintPolicy = (selection) => {
    const manual = selection.kind !== "blocked" && !selection.wssOnly;
    policyStatus.hidden = manual;
    status.hidden = !manual;
    for (const control of manualControls) control.hidden = !manual;
    if (manual) return false;
    remove.hidden = true;
    permission.hidden = !selection.origin || !httpNetworkPermission(selection.origin);
    if (selection.kind === "blocked") policyStatus.textContent = transportReasonText(selection.reason);
    else {
      const source = selection.source === "hosted" ? "Hosted HTTPS" : "Personal HTTPS";
      const terminal = terminalCompatibilityText.get(cachedTerminalTransport(deviceId));
      policyStatus.textContent = `${source} · ${selection.origin} · ${terminal}`;
    }
    return true;
  };
  const paint = () => {
    if (!active || paintPolicy(deviceTransportFor(deviceId))) return;
    const preference = httpPreferenceFor(deviceId);
    select.value = preference?.selected || "webrtc";
    select.querySelector('[value="http"]').disabled = !preference;
    remove.hidden = !preference;
    const origin = preference?.origin || "";
    if (origin !== shownOrigin) { address.value = origin; shownOrigin = origin; }
    paintStatus();
  };
  const stop = onHttpPreferencesChanged(paint);
  const stopDevices = onDevicesChanged(paint);
  const stopStatus = onDeviceStateChanged(paintStatus);
  void restoreHttpConnections().then(paint).catch(() => {});
  paint();
  const change = async (options) => {
    select.disabled = true; status.textContent = "Changing transport…";
    try {
      await chooseDeviceTransport(deviceId, options);
      if (active) status.textContent = "Selected transport connected.";
    } catch (error) { if (active) status.textContent = httpFailure(error?.code || "network").message; }
    finally { if (active) { select.disabled = false; paint(); } }
  };
  select.onchange = () => change({ transport: select.value, identity: httpPreferenceFor(deviceId) });
  form.onsubmit = (event) => {
    event.preventDefault();
    const device = App.devices.find((item) => item.id === deviceId);
    const identity = { deviceId, origin: address.value, fingerprint: device?.fingerprint };
    return change({ transport: "http", identity });
  };
  remove.onclick = () => change({ transport: "webrtc", removeAddress: true });
  permission.onclick = async () => {
    const selection = deviceTransportFor(deviceId);
    if (selection.kind !== "http") return;
    await requestHttpNetworkPermission(selection.origin);
    if (active) void connectDevice(deviceId, { reason: "browser-network-permission" }).catch(() => {});
  };
  return () => { active = false; stop(); stopStatus(); stopDevices(); };
}
