// Add a device: enter the pairing code the bridge printed, verify the fingerprint
// out-of-band, then approve — binding the device to your account.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { lookupDevice, approveDevice } from "../api.js";

export function openAddDevice(onDone) {
  $("#sheet").innerHTML = `
    <h3>Add a device</h3><div class="sub">Enter the pairing code your bridge printed on startup.</div>
    <input id="paircode" placeholder="e.g. WXYZ-4F2K" style="text-transform:uppercase" />
    <div class="row"><button class="btn" id="pcancel" style="margin-left:auto">Cancel</button>
      <button class="btn primary" id="plookup">Look up</button></div>
    <div id="pairbox"></div>
    <div class="adderr" id="perr"></div>`;
  $("#scrim").classList.add("show");
  $("#paircode").focus();
  $("#pcancel").onclick = () => $("#scrim").classList.remove("show");
  $("#plookup").onclick = async () => {
    const code = $("#paircode").value.trim().toUpperCase();
    if (!code) return;
    $("#perr").textContent = "";
    let device;
    try {
      device = await lookupDevice(code);
    } catch (e) {
      $("#perr").textContent = e.message;
      return;
    }
    $("#pairbox").innerHTML = `
      <div class="panel" style="margin-top:12px">
        <div class="row"><span class="k">Device</span><span class="v">${esc(device.name)}</span></div>
        <div class="row"><span class="k">Fingerprint</span><span class="v mono" style="font-size:11px;word-break:break-all">${esc(device.fingerprint)}</span></div>
        <div class="dim" style="font-size:12px;margin:8px 0">Confirm this matches the fingerprint the bridge printed, then approve.</div>
        <button class="btn primary" id="papprove">Approve &amp; pair</button></div>`;
    $("#papprove").onclick = async () => {
      $("#papprove").disabled = true;
      $("#perr").textContent = "";
      try {
        await approveDevice(code);
        $("#scrim").classList.remove("show");
        await onDone();
      } catch (e) {
        $("#perr").textContent = e.message;
        $("#papprove").disabled = false;
      }
    };
  };
}
