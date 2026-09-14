// What this client says about a machine it cannot reach — offline, or never
// opened here: the device's name, and why there is nothing under it. The route
// hosts stand it up in place of a surface; an opener that will not open says the
// same sentence out loud. It is worded, marked up and taken down again once,
// here.

import { App, render } from "../app.js";
import { deviceOfflineText, esc } from "./text.js";
import { deviceNameOf } from "./devicePolicy.js";
import { canAnswer, onDeviceStateChanged, routeContext } from "./deviceContexts.js";

/** What this client says about a machine it cannot reach, in the account's name
 *  for it — the one sentence, whether a surface prints it or a refused opener
 *  says it out loud. */
export const deviceOfflineNotice = (deviceId) => deviceOfflineText(deviceNameOf(App.devices, deviceId));

/** The notice a link to an unreachable device stands in for a surface with. */
export const deviceOfflineHtml = (deviceId) => `<div class="empty">${esc(deviceOfflineNotice(deviceId))}</div>`;

/**
 * Stand the notice up where a surface would go, and take it down again the
 * moment its machine can answer.
 *
 * A link is not dead for naming a machine that is not here yet: the devices land
 * one at a time, so a reload paints on whichever answered first and the link's
 * own machine arrives a beat later — as does one that has been out and resumes.
 * Nothing about the route changed when it did, so the surface would sit on the
 * notice until the reader navigated away and back. Waiting for the device is
 * this view's whole behaviour, so it owns the teardown: App.viewDispose is the
 * unsubscribe, which the next render runs.
 */
export function mountDeviceNotice(root, deviceId) {
  root.innerHTML = deviceOfflineHtml(deviceId);
  App.viewDispose = onDeviceStateChanged(() => {
    if (canAnswer(routeContext(App.route))) render();
  });
}
