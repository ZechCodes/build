// Add a device: enter the pairing code the bridge printed, verify the fingerprint
// out-of-band, then approve — binding the device to your account.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { lookupDevice, approveDevice } from "../api.js";
import { settingsSheetHtml } from "./settingsSheet.js";
import { fieldTraits } from "../core/fieldTraits.js";

export function openAddDevice(onDone) {
  const sheet = $("#sheet");
  const scrim = $("#scrim");
  sheet.innerHTML = settingsSheetHtml({
    title: "Add a device",
    subtitleHtml: "Enter the pairing code your bridge printed on startup.",
    bodyHtml: `
    <input id="paircode" ${fieldTraits("code")} placeholder="e.g. WXYZ-4F2K" style="text-transform:uppercase" />
    <div class="row"><button class="btn" id="pcancel" style="margin-left:auto">Cancel</button>
      <button class="btn primary" id="plookup">Look up</button></div>
    <div id="pairbox"></div>
    <div class="adderr" id="perr"></div>`,
  });
  const input = sheet.querySelector("#paircode");
  const error = sheet.querySelector("#perr");
  const pairbox = sheet.querySelector("#pairbox");
  let active = true;
  let lookupVersion = 0;
  const ownsSheet = () => sheet.contains(input);
  const current = () => active && ownsSheet() && scrim.classList.contains("show");
  const dispose = () => {
    active = false;
    lookupVersion += 1;
    if (ownsSheet()) scrim.classList.remove("show");
  };
  scrim.classList.add("show");
  input.focus();
  sheet.querySelector("#pcancel").onclick = dispose;
  sheet.querySelector("#plookup").onclick = async () => {
    if (!current()) return;
    const code = input.value.trim().toUpperCase();
    if (!code) return;
    const version = ++lookupVersion;
    error.textContent = "";
    pairbox.replaceChildren();
    let device;
    try {
      device = await lookupDevice(code);
    } catch (e) {
      if (current() && version === lookupVersion) error.textContent = e.message;
      return;
    }
    if (!current() || version !== lookupVersion) return;
    pairbox.innerHTML = `
      <div class="panel" style="margin-top:12px">
        <div class="row"><span class="k">Device</span><span class="v">${esc(device.name)}</span></div>
        <div class="row"><span class="k">Fingerprint</span><span class="v mono" style="font-size:11px;word-break:break-all">${esc(device.fingerprint)}</span></div>
        <div class="dim" style="font-size:12px;margin:8px 0">Confirm this matches the fingerprint the bridge printed, then approve.</div>
        <button class="btn primary" id="papprove">Approve &amp; pair</button></div>`;
    const approve = pairbox.querySelector("#papprove");
    approve.onclick = async () => {
      if (!current() || version !== lookupVersion || approve.disabled) return;
      approve.disabled = true;
      error.textContent = "";
      try {
        await approveDevice(code);
        // Pairing already succeeded even if navigation replaced this sheet.
        // Notify its caller, but never close a newer sheet on its behalf.
        dispose();
        await onDone();
      } catch (e) {
        if (current() && version === lookupVersion) {
          error.textContent = e.message;
          approve.disabled = false;
        }
      }
    };
  };
  return dispose;
}
