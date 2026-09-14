// What this client says about a machine it cannot reach — offline, or never
// opened here: the device's name, and why there is nothing under it. The route
// hosts stand it up in place of a surface; an opener that will not open says the
// same sentence out loud. It is worded, marked up and taken down again once,
// here.

import { App, render } from "../app.js";
import { deviceFrozenText, deviceOfflineText, esc } from "./text.js";
import { deviceNameOf } from "./devicePolicy.js";
import { canAnswer, onDeviceStateChanged, routeContext } from "./deviceContexts.js";

/** What this client says about a machine it cannot reach, in the account's name
 *  for it — the one sentence, whether a surface prints it or a refused opener
 *  says it out loud. */
export const deviceOfflineNotice = (deviceId) => deviceOfflineText(deviceNameOf(App.devices, deviceId));

/** What this client says over a surface that was already open when its machine
 *  went: it keeps what that machine last said, so it names whose state it is
 *  showing rather than offering to open anything. */
const deviceFrozenNotice = (deviceId) => deviceFrozenText(deviceNameOf(App.devices, deviceId));

/** The notice a link to an unreachable device stands in for a surface with. */
const deviceOfflineHtml = (deviceId) => `<div class="empty">${esc(deviceOfflineNotice(deviceId))}</div>`;

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

/** What a surface whose machine cannot answer says over itself. The frozen
 *  sentence is a promise about what is on screen — this is what that machine
 *  last said — so it is only for a surface with something on it. One whose
 *  machine went before its first read landed has nothing but "loading…" over a
 *  frame that never filled, and says the plain thing instead. */
const awayWords = (deviceId, hasContent) =>
  hasContent() ? deviceFrozenNotice(deviceId) : deviceOfflineNotice(deviceId);

/**
 * Name the machine over a surface that is open when it goes, and stop naming it
 * when that machine answers again.
 *
 * The surface itself stays exactly as it was read: there is nothing to hand
 * back — the reader is already standing on it — and the panes below keep the
 * last state the machine described. All that is missing is whose state it is,
 * which is one strip over the top and a mark on the host so what claims to be
 * live can stop claiming it. The mounting surface owns the teardown.
 *
 * `hasContent` is the surface's own answer to "is there anything on me yet":
 * one that is only ever stood up over what it read says nothing and takes the
 * default, and one that can still be sitting on its loading frame answers for
 * itself.
 */
export function mountDeviceStrip(host, context, { hasContent = () => true } = {}) {
  const paint = () => nameTheMachine(host, canAnswer(context) ? null : awayWords(context.deviceId, hasContent));
  paint();
  const stopListening = onDeviceStateChanged(paint);
  return () => {
    stopListening();
    nameTheMachine(host, null);
  };
}

/** One strip or none: the host carries at most one, whatever the account says
 *  and however often it says it. */
function nameTheMachine(host, words) {
  host.classList.toggle("device-away", Boolean(words));
  const shown = host.querySelector(":scope > .device-strip");
  if (!words) {
    shown?.remove();
    return;
  }
  const strip = shown || host.insertAdjacentElement("afterbegin", document.createElement("div"));
  strip.className = "device-strip";
  strip.textContent = words;
}
