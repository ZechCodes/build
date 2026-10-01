// A pending device's fingerprint as the approve screens show it: the short
// form `build-bridge pair` prints (its first 32 hex digits, 128 bits, in
// fours) to compare, and the whole of it beneath.

import { esc } from "./text.js";

export function shortFingerprint(fingerprint) {
  return (String(fingerprint).slice(0, 32).match(/.{1,4}/g) || []).join(" ");
}

/** The device being approved and its fingerprint, ending on the approve button
 *  `buttonId`. */
export function pendingDeviceHtml(device, buttonId) {
  return `
      <div class="panel" style="margin-top:12px">
        <div class="row"><span class="k">Device</span><span class="v">${esc(device.name)}</span></div>
        <div class="row"><span class="k">Fingerprint</span><span class="v mono" data-fingerprint-short>${esc(shortFingerprint(device.fingerprint))}</span></div>
        <div class="dim mono" data-fingerprint-full style="font-size:10.5px;word-break:break-all;margin-top:4px">${esc(device.fingerprint)}</div>
        <div class="dim" style="font-size:12px;margin:8px 0">Confirm this matches the fingerprint the bridge printed, then approve.</div>
        <button class="btn primary" id="${buttonId}">Approve &amp; pair</button></div>`;
}
