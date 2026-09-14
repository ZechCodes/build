// What this client says about a machine it cannot reach — offline, or never
// opened here: the device's name, and why there is nothing under it. The route
// hosts paint it in place of a surface; an opener that will not open says the
// same sentence out loud. It is worded and marked up once, here.

import { App } from "../app.js";
import { deviceOfflineText, esc } from "./text.js";
import { deviceNameOf } from "./devicePolicy.js";

/** What this client says about a machine it cannot reach, in the account's name
 *  for it — the one sentence, whether a surface prints it or a refused opener
 *  says it out loud. */
export const deviceOfflineNotice = (deviceId) => deviceOfflineText(deviceNameOf(App.devices, deviceId));

/** The notice a link to an unreachable device stands in for a surface with. */
export const deviceOfflineHtml = (deviceId) => `<div class="empty">${esc(deviceOfflineNotice(deviceId))}</div>`;
