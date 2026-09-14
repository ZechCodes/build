// What a work surface shows in place of itself when the machine its link names
// cannot answer — offline, or never opened on this client: the device's name,
// and why there is nothing under it. Both route hosts — the branch surface and
// the issue surface — paint the same notice, so it is worded and marked up
// once, here.

import { App } from "../app.js";
import { deviceOfflineText, esc } from "./text.js";
import { deviceNameOf } from "./devicePolicy.js";

/** The notice a link to an unreachable device stands in for a surface with. */
export const deviceOfflineHtml = (deviceId) =>
  `<div class="empty">${esc(deviceOfflineText(deviceNameOf(App.devices, deviceId)))}</div>`;
