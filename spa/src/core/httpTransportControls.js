import { httpPreferenceFor, onHttpPreferencesChanged, restoreHttpConnections } from "./httpConnections.js";
import { chooseDeviceTransport, connectDevice } from "../connection.js";
import { App } from "../appState.js";
import { contextFor, onDeviceStateChanged } from "./deviceContexts.js";
import { httpFailure } from "./httpEndpoint.js";
import { httpNetworkPermission, requestHttpNetworkPermission } from "./httpNetworkPermission.js";

const transportProblems = new Map([
  ["http-sign-in", "sign_in"],
  ["http-wrong-bridge", "wrong_bridge"],
  ["http-permission", "network_permission"],
  ["http-permission-denied", "network_permission_denied"],
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
    <form data-http-address><label for="device-http-address">Bridge HTTPS address</label>
      <div class="projfolder"><input id="device-http-address" type="url" inputmode="url" autocomplete="off" required>
      <button class="btn" type="submit">Save and use HTTPS</button></div></form>
    <p class="dim">Terminals also use HTTPS when supported. Older setups may use WebRTC. Transport changes do not resend uncertain actions.</p>
    <button class="btn" type="button" data-http-remove hidden>Remove HTTPS address</button>
    <p role="status" aria-live="polite" data-http-status></p>
    <button class="btn" type="button" data-http-permission hidden>Allow browser network access</button>`;
  const select = host.querySelector("select");
  const form = host.querySelector("form");
  const address = form.querySelector("input");
  const remove = host.querySelector("[data-http-remove]");
  const permission = host.querySelector("[data-http-permission]");
  const status = host.querySelector("[data-http-status]");
  let active = true;
  let shownOrigin;
  const paintStatus = () => {
    if (!active) return;
    const context = contextFor(deviceId);
    const chosen = httpPreferenceFor(deviceId);
    if (chosen?.selected !== "http") { permission.hidden = true; return; }
    const problem = transportProblems.get(context?.blocked);
    if (!context?.session && !context?.blocked) return;
    status.textContent = problem ? httpFailure(problem).message : "";
    permission.hidden = !showPermission(problem, chosen.origin);
  };
  const paint = () => {
    if (!active) return;
    const preference = httpPreferenceFor(deviceId);
    select.value = preference?.selected || "webrtc";
    select.querySelector('[value="http"]').disabled = !preference;
    remove.hidden = !preference;
    const origin = preference?.origin || "";
    if (origin !== shownOrigin) { address.value = origin; shownOrigin = origin; }
    paintStatus();
  };
  const stop = onHttpPreferencesChanged(paint);
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
    const preference = httpPreferenceFor(deviceId);
    if (preference?.selected !== "http") return;
    await requestHttpNetworkPermission(preference.origin);
    if (active) void connectDevice(deviceId, { reason: "browser-network-permission" }).catch(() => {});
  };
  return () => { active = false; stop(); stopStatus(); };
}
