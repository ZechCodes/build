import { httpPreferenceFor, onHttpPreferencesChanged, restoreHttpConnections } from "./httpConnections.js";
import { chooseDeviceTransport } from "../connection.js";
import { openAddByUrl } from "../sheets/addByUrl.js";
import { httpFailure } from "./httpEndpoint.js";

// This is saved configuration. Reachability never controls the surrounding
// settings rendering; a failed HTTPS choice remains explicit and reversible.
export function mountHttpTransportControls(host, deviceId) {
  host.innerHTML = `<h3>Connection transport</h3>
    <label for="device-transport">Views and actions</label>
    <select id="device-transport"><option value="webrtc">WebRTC</option><option value="http">HTTPS and live updates</option></select>
    <p class="dim" data-http-origin></p><p class="dim">HTTPS is optional. The terminal always uses WebRTC separately. Transport changes do not resend uncertain actions.</p>
    <button class="btn" data-http-add>Verify HTTPS address…</button><p role="status" aria-live="polite" data-http-status></p>`;
  const select = host.querySelector("select");
  const status = host.querySelector("[data-http-status]");
  let active = true;
  let disposeSheet;
  const paint = () => {
    if (!active) return;
    const preference = httpPreferenceFor(deviceId);
    select.value = preference?.selected || "webrtc";
    select.querySelector('[value="http"]').disabled = !preference;
    host.querySelector("[data-http-origin]").textContent = preference?.origin || "No verified HTTPS address saved.";
  };
  const stop = onHttpPreferencesChanged(paint);
  void restoreHttpConnections().then(paint).catch(() => {});
  paint();
  select.onchange = async () => {
    const selected = select.value;
    select.disabled = true; status.textContent = "Changing transport…";
    try {
      await chooseDeviceTransport(deviceId, { transport: selected, identity: httpPreferenceFor(deviceId) });
      if (active) status.textContent = "Selected transport connected.";
    } catch (error) { if (active) status.textContent = httpFailure(error?.code || "network").message; }
    finally { if (active) { select.disabled = false; paint(); } }
  };
  host.querySelector("[data-http-add]").onclick = () => { disposeSheet?.(); disposeSheet = openAddByUrl(paint); };
  return () => { active = false; stop(); disposeSheet?.(); };
}
