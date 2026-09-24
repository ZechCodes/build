// What this client says about a machine it cannot reach — offline, or never
// opened here: the device's name, and why there is nothing under it. The route
// hosts stand it up in place of a surface; an opener that will not open says the
// same sentence out loud. It is worded, marked up and taken down again once,
// here.

import { App, render } from "../app.js";
import { deviceFrozenText, esc } from "./text.js";
import { deviceAwayText } from "./deviceAway.js";
import { deviceNameOf } from "./devicePolicy.js";
import { canAnswer, contextFor, onDeviceStateChanged } from "./deviceContexts.js";
import { surfaceContext } from "./surfaceContext.js";
import { connectDevice, deviceRecoverySnapshot, onDeviceRecoveryChanged } from "../connection.js";

/** What this client says about a machine it cannot reach, in the account's name
 *  for it — the one sentence, whether a surface prints it or a refused opener
 *  says it out loud. */
export function deviceOfflineNotice(deviceId) {
  return deviceAwayText(contextFor(deviceId), deviceNameOf(App.devices, deviceId));
}

/** What this client says over a surface that was already open when its machine
 *  went: it keeps what that machine last said, so it names whose state it is
 *  showing rather than offering to open anything. */
const deviceFrozenNotice = (deviceId) => deviceFrozenText(deviceNameOf(App.devices, deviceId));

/** The notice a link to an unreachable device stands in for a surface with. */
const deviceOfflineHtml = (deviceId) => `<div class="empty">${esc(deviceOfflineNotice(deviceId))}</div>`;

const listedOnline = (deviceId) => App.devices.some((device) => device.id === deviceId && device.status === "online");
const recovering = (deviceId) => {
  const state = deviceRecoverySnapshot(deviceId);
  return listedOnline(deviceId) && state && !Array.isArray(state) && state.status !== "idle";
};

/**
 * Stand the notice up where a surface would go, and take it down again the
 * moment the surface has a machine to stand on (core/surfaceContext.js) — the
 * one question every route surface asks before it paints.
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
  const paint = () => {
    if (surfaceContext(App.route)) return render();
    const context = contextFor(deviceId);
    const connecting = listedOnline(deviceId) && (!context?.blocked || recovering(deviceId));
    root.innerHTML = connecting
      ? `<div class="empty">Connecting to ${esc(deviceNameOf(App.devices, deviceId) || "device")}…</div>`
      : deviceOfflineHtml(deviceId);
    const askAgain = connecting ? null : retryControl(deviceId);
    if (askAgain) root.querySelector(".empty").append(" ", askAgain);
  };
  paint();
  const stops = [onDeviceStateChanged(paint), onDeviceRecoveryChanged(paint)];
  App.viewDispose = () => stops.forEach((stop) => stop());
}

/** What a surface whose machine cannot answer says over itself. The frozen
 *  sentence is a promise about what is on screen — this is what that machine
 *  last said — so it is only for a surface with something on it. One whose
 *  machine went before its first read landed has nothing but "loading…" over a
 *  frame that never filled, and says the plain thing instead. */
const awayWords = (deviceId, hasContent) =>
  hasContent() && !contextFor(deviceId)?.blocked ? deviceFrozenNotice(deviceId) : deviceOfflineNotice(deviceId);

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
  const paint = () =>
    nameTheMachine(
      host,
      canAnswer(context) || recovering(context.deviceId) ? null : awayWords(context.deviceId, hasContent),
      context.deviceId,
    );
  paint();
  const stops = [onDeviceStateChanged(paint), onDeviceRecoveryChanged(paint)];
  return () => {
    stops.forEach((stop) => stop());
    nameTheMachine(host, null);
  };
}

/** One strip or none: the host carries at most one, whatever the account says
 *  and however often it says it. */
function nameTheMachine(host, words, deviceId) {
  host.classList.toggle("device-away", Boolean(words));
  const shown = host.querySelector(":scope > .device-strip");
  if (!words) {
    shown?.remove();
    return;
  }
  const strip = shown || host.insertAdjacentElement("afterbegin", document.createElement("div"));
  strip.className = "device-strip";
  strip.replaceChildren(words);
  const askAgain = retryControl(deviceId);
  if (askAgain) strip.append(" ", askAgain);
}

/**
 * The one thing a reader can do about a machine nothing could reach: ask again.
 *
 * Only a blocked machine has one (spec rule 3). A machine that is merely away
 * comes back when its bridge does — the presence poll opens it then — and a
 * button that only waited would be a promise this client cannot keep.
 */
function retryControl(deviceId) {
  if (!contextFor(deviceId)?.blocked) return null;
  const button = document.createElement("button");
  button.type = "button";
  button.className = "btn mini device-retry";
  button.dataset.retryDevice = deviceId;
  button.textContent = "Retry";
  button.onclick = () => {
    button.disabled = true;
    // However it settles, the registry says what became of that machine: a
    // machine that answered takes its own strip down, and one that could not be
    // reached again repaints this one with the new reason.
    connectDevice(deviceId, { reason: "row-retry" }).catch(() => {}).finally(() => {
      button.disabled = false;
    });
  };
  return button;
}
