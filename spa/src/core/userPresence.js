// The user arriving at this client, told to every bridge it is connected to.
//
// "Done since you left" starts from the user's last session on a bridge, and a
// session starts with the user doing something there. Looking counts: coming
// back to the window (it is focused again, or its tab shown while focused),
// touching or typing in it, moving to another page of it, or opening the app
// in a window that has focus. Each of those is an arrival, and the bridge
// records it on its own clock (`user.present`), so a reload, or a second
// client, reads the same session start rather than guessing one.
//
// Nothing automatic is an arrival: a reconnect, a list read, a push. A window
// left focused overnight sends nothing until someone comes back to it.

import { bridgeCapabilities } from "./changeEvents.js";
import { liveContexts, onDeviceStateChanged, whenGreeted } from "./deviceContexts.js";
import { onReaderReturns, readerIsHere } from "./readerPresence.js";
import { writeUserSession } from "./userSessionCache.js";

/** At most one `user.present` per bridge per minute: the session only has to
 *  hear of the user once in six hours, and a pointer is busy. */
export const PRESENCE_EVERY_MS = 60_000;

/** An arrival a bridge could not be told of (it was not connected) is told
 *  when it connects, if that is soon. A bridge that comes back at 3am must
 *  not be told the user arrived at 3am. */
export const ARRIVAL_FRESH_MS = 2 * 60_000;

const INPUT_EVENTS = ["pointerdown", "keydown", "wheel"];

let arrivedAt = null;
/** deviceId → when this client last told that bridge, on this device's clock. */
const told = new Map();
let stopPresence = null;

const carriesPresence = (deviceId) => bridgeCapabilities(deviceId)?.tasks?.doneSinceLeft === true;

function arrived() {
  if (!readerIsHere()) return;
  arrivedAt = Date.now();
  tellBridges();
}

/** Whether there is an arrival recent enough to tell anyone of. */
const arrivalIsFresh = () => arrivedAt !== null && Date.now() - arrivedAt <= ARRIVAL_FRESH_MS;

/** Tell every connected bridge that has not heard of the latest arrival. */
function tellBridges() {
  const now = Date.now();
  if (!arrivalIsFresh()) return;
  for (const context of liveContexts()) {
    const last = told.get(context.deviceId);
    if (last !== undefined && (last >= arrivedAt || now - last < PRESENCE_EVERY_MS)) continue;
    told.set(context.deviceId, now);
    void tell(context, now);
  }
}

/** Once the bridge has greeted (which says whether it records arrivals), tell
 *  it. The greeting can settle long after the arrival (a suspended tab), so
 *  the arrival is checked again then: still fresh, and someone still at the
 *  window. The answer is the session the arrival left: written to the cache
 *  like any list read's, so the Dashboard repaints from it. A bridge that does
 *  not record them, or refuses, is asked again at the next arrival. */
async function tell(context, at) {
  const forget = () => { if (told.get(context.deviceId) === at) told.delete(context.deviceId); };
  const request = await whenGreeted(context, () => {
    if (!arrivalIsFresh() || !readerIsHere() || !carriesPresence(context.deviceId)) return null;
    return context.rpc("user.present", {});
  }).catch(() => null);
  if (!request?.sent) return forget();
  const answer = await request.sent.catch(() => null);
  if (!answer || !request.stands()) return forget();
  return writeUserSession(context.deviceId, answer, request.stands);
}

/** Listen for arrivals, and count this page load as one if the window has
 *  focus. Returns the stop; a second start replaces the first. */
export function startUserPresence() {
  stopPresence?.();
  const doc = globalThis.document;
  const win = globalThis.window;
  const stopReturns = onReaderReturns(arrived);
  const stopDevices = onDeviceStateChanged(tellBridges);
  for (const name of INPUT_EVENTS) doc?.addEventListener(name, arrived, { capture: true, passive: true });
  win?.addEventListener("hashchange", arrived);
  arrived();
  stopPresence = () => {
    stopReturns();
    stopDevices();
    for (const name of INPUT_EVENTS) doc?.removeEventListener(name, arrived, { capture: true });
    win?.removeEventListener("hashchange", arrived);
    stopPresence = null;
  };
  return stopPresence;
}

/** Tests: forget every arrival and every bridge told. */
export function resetUserPresence() {
  stopPresence?.();
  arrivedAt = null;
  told.clear();
}
