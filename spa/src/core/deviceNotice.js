// What a work surface shows in place of itself when the machine its link names
// has no session on this client: the device's name, and why there is nothing
// under it. Both route hosts — the branch surface and the issue surface — paint
// the same notice, so it is worded and marked up once, here.

import { App } from "../app.js";
import { esc, unopenedDeviceText } from "./text.js";
import { deviceNameOf } from "./devicePolicy.js";

/** The notice a link to an unopened device stands in for a surface with. */
export const unopenedDeviceHtml = (deviceId) =>
  `<div class="empty">${esc(unopenedDeviceText(deviceNameOf(App.devices, deviceId)))}</div>`;
