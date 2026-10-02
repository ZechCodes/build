// Add a device: enter the pairing code the bridge printed, verify the fingerprint
// out-of-band, then approve — binding the device to your account. Opened from
// the bridge's approve link, the code arrives filled in and is looked up at
// once; approving is still the reader's press.

import { $ } from "../dom.js";
import { lookupDevice, approveDevice } from "../api.js";
import { settingsSheetHtml } from "./settingsSheet.js";
import { fieldTraits } from "../core/fieldTraits.js";
import { pendingDeviceHtml } from "../core/deviceFingerprint.js";

/** A link opened the sheet: anyone can send one, so it says so and what
 *  approving does before the device it names can be approved (#319). */
const LINK_ARRIVAL = {
  subtitleHtml: "A pairing link opened this.",
  warningHtml: `<div class="addwarn" data-link-warning>Only approve if you just ran the installer or
    <code>build-bridge pair</code> on a machine you own. Approving gives that machine access to your account.</div>`,
};
const TYPED = { subtitleHtml: "Enter the pairing code your bridge printed on startup.", warningHtml: "" };

export function openAddDevice(onDone, { code = "", fromLink = false } = {}) {
  const sheet = $("#sheet");
  const scrim = $("#scrim");
  const arrival = fromLink ? LINK_ARRIVAL : TYPED;
  sheet.innerHTML = settingsSheetHtml({
    title: "Add a device",
    subtitleHtml: arrival.subtitleHtml,
    bodyHtml: `${arrival.warningHtml}
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
  const lookup = async () => {
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
    pairbox.innerHTML = pendingDeviceHtml(device, "papprove");
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
  sheet.querySelector("#plookup").onclick = lookup;
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.isComposing) lookup();
  });
  if (code) {
    input.value = code.toUpperCase();
    void lookup();
  }
  return dispose;
}
