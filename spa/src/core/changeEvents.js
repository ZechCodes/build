// Push invalidation, client side.
//
// Every detail surface used to poll on a 1.6–2s timer because nothing told it
// anything had happened. A bridge that pushes change events tells it, so the
// timers stand down to a safety poll and the refetch happens when the state
// actually moved.
//
// Two events come off the session (the bridge's wire, unchanged here):
//
//   {"type":"board.changed"}                  the feed is stale
//   {"type":"entity.changed","id":"run-7"}    one entity's detail is stale
//
// Nothing about WHAT changed rides the wire — the client refetches what it is
// already showing, which is the only thing it could render anyway.
//
// # Two modes, and the old bridge
//
// Event mode arms only on `push_events: true` in the `session.hello` greeting.
// A bridge that predates push invalidation refuses that method, the greeting
// yields nothing, and every surface keeps the interval it has always had. That
// fallback is not a degraded path: it is today's behaviour, unchanged, and the
// deployed client has to keep working against a bridge nobody has updated.
//
// # One paint, whatever woke it
//
// A surface registers its EXISTING poll callback here. An event runs that same
// callback — the same freeze guards, the same render-key comparison, the same
// keyed list patch — so a push and a tick end in the same paint. There is no
// second refresh path to keep in step with the first.

import { pageVisible, whenVisible } from "./visibility.js";

/** How often an armed surface still reads on its own. Events do the work; this
 *  is what catches whatever an event never covered — a bridge restart, a
 *  coalesced flush lost to a socket hiccup — without being a poll anyone waits
 *  on. */
export const SAFETY_POLL_MS = 60000;

// Event mode is a fact about one bridge, so it is held per device: one machine
// pushing says nothing about another that predates push invalidation.
const armed = new Map(); // deviceId → whether that bridge pushes
const watchers = new Set();
let visibilityWired = false;

/** A device id as this module keys it. A caller with no device to name (a
 *  surface that spans them, a test) is the unnamed one. */
const deviceKeyOf = (deviceId) => (deviceId === null || deviceId === undefined ? "" : String(deviceId));

/** Read one bridge's greeting. Returns whether event mode armed for it. */
export function armChangeEvents(greeting, deviceId = null) {
  const device = deviceKeyOf(deviceId);
  const nowArmed = Boolean(greeting && greeting.push_events === true);
  if (armed.get(device) === nowArmed) return nowArmed;
  armed.set(device, nowArmed);
  // A surface mounted before the mode was known (or across a reconnect onto a
  // different bridge) keeps polling at whatever cadence it started with unless
  // it is re-timed here.
  retime(device);
  return nowArmed;
}

/** Retire a device: what it pushed is nobody's cadence any more. */
export function disarmChangeEvents(deviceId) {
  const device = deviceKeyOf(deviceId);
  if (!armed.delete(device)) return;
  retime(device);
}

/** Re-time every watcher that hears this device — its own, and the ones that
 *  span devices and therefore follow every bridge's mode. */
function retime(device) {
  [...watchers].filter((watcher) => watcher.hears(device)).forEach(startTimer);
}

/** Whether pushes can be expected. For one device, that device's bridge; for a
 *  surface that spans devices, only when every bridge it could hear from
 *  pushes — one polling device is a device nothing would announce. */
export function changeEventsArmed(deviceId = null) {
  if (deviceId === null || deviceId === undefined) {
    return armed.size > 0 && [...armed.values()].every(Boolean);
  }
  return armedFor(deviceId);
}

/** One bridge's mode, with no spanning rule over it: what an event arriving
 *  from that device is measured against. */
const armedFor = (deviceId) => armed.get(deviceKeyOf(deviceId)) === true;

/** The interval a surface polling every `fastMs` should actually run at. Event
 *  mode stands a fast poll down to the safety poll and leaves a slow one alone —
 *  standing down must never mean speeding up. */
export function pollIntervalMs(fastMs, deviceId = null) {
  return changeEventsArmed(deviceId) ? Math.max(fastMs, SAFETY_POLL_MS) : fastMs;
}

/** Forget every watcher and disarm every device. Tests, and a client that lost
 *  its sessions. */
export function resetChangeEvents() {
  watchers.forEach((watcher) => clearInterval(watcher.timer));
  watchers.clear();
  armed.clear();
}

/** The ids a watcher stands for right now. Read at delivery, never at mount:
 *  a surface whose entity resolves late (a branch view learns its run id from
 *  the first read) must be asked about the entity it is showing now. */
function entityIdsOf(watcher) {
  const named = typeof watcher.entity === "function" ? watcher.entity() : watcher.entity;
  const list = Array.isArray(named) ? named : [named];
  return list.filter((id) => id !== null && id !== undefined && id !== "").map(String);
}

/** The predicate a watcher is registered with: the whole of "does this surface
 *  hear that device?", asked once per delivery and never re-derived. */
const hearsFor = (deviceId) =>
  deviceId === null || deviceId === undefined ? () => true : (device) => deviceKeyOf(device) === deviceKeyOf(deviceId);

function startTimer(watcher) {
  clearInterval(watcher.timer);
  watcher.timer = setInterval(watcher.tick, pollIntervalMs(watcher.intervalMs, watcher.deviceId));
}

/** Run a watcher's refresh for an event, under the same visibility gate its
 *  poll runs under. A hidden tab notes that it owes a refetch instead. */
function deliver(watcher) {
  if (watcher.pausesWhileHidden && !pageVisible()) {
    watcher.missed = true;
    return;
  }
  watcher.missed = false;
  watcher.refresh();
}

/** Coming back to a tab that was told about changes it could not act on. The
 *  poll's own visible-again catch-up, for pushes. */
function onVisibilityChange() {
  if (!pageVisible()) return;
  [...watchers].forEach((watcher) => {
    if (!watcher.missed) return;
    watcher.missed = false;
    if (watcher.catchUpOnVisible) watcher.refresh();
  });
}

/**
 * Register a polling surface.
 *
 * `refresh` is the surface's existing poll callback — the one whose paint the
 * whole surface is built around. `intervalMs` is the cadence it polls at while
 * nothing is pushing; event mode re-times it to the safety poll. `entity` names
 * what the surface is showing (a string, or a function returning a string or a
 * list); leaving it out makes the surface board-scoped — it is the feed, and
 * `board.changed` is its event.
 *
 * `catchUpOnVisible: false` is for a surface that already refreshes itself on
 * visibilitychange (the feed does), so coming back does not read twice.
 * `pausesWhileHidden: false` is for a surface whose poll is not visibility-gated
 * — its events are not gated either, because parity with the poll is the rule.
 *
 * Returns `{ dispose }`; it owns the interval, so the caller stops clearing one.
 */
export function watchChanges({
  refresh,
  intervalMs,
  entity = null,
  deviceId = null,
  catchUpOnVisible = true,
  pausesWhileHidden = true,
}) {
  const watcher = {
    refresh,
    intervalMs,
    entity,
    deviceId,
    catchUpOnVisible,
    pausesWhileHidden,
    // Whose events this surface is about, decided once, here: a surface that
    // named a device hears that device, and one that named none spans them all.
    hears: hearsFor(deviceId),
    // A surface that named no entity is the feed, whatever its entity getter
    // would answer later.
    boardScoped: entity === null || entity === undefined,
    missed: false,
    timer: null,
    tick: pausesWhileHidden ? whenVisible(refresh) : refresh,
  };
  watchers.add(watcher);
  startTimer(watcher);
  if (!visibilityWired && typeof document !== "undefined") {
    document.addEventListener("visibilitychange", onVisibilityChange);
    visibilityWired = true;
  }
  return {
    dispose() {
      clearInterval(watcher.timer);
      watcher.timer = null;
      watchers.delete(watcher);
    },
  };
}

/** A change event off the session. Ignored entirely while unarmed — an old
 *  bridge sends none, and a client that never greeted must behave as if it
 *  could not hear them. Returns whether the event was one we act on. */
export function dispatchChangeEvent(payload, deviceId = null) {
  if (!payload || !armedFor(deviceId)) return false;
  if (payload.type === "board.changed") {
    [...watchers].filter((watcher) => watcher.boardScoped && watcher.hears(deviceId)).forEach(deliver);
    return true;
  }
  if (payload.type === "entity.changed") {
    const id = payload.id === null || payload.id === undefined ? "" : String(payload.id);
    if (!id) return false;
    [...watchers].filter((watcher) => entityIdsOf(watcher).includes(id)).forEach(deliver);
    return true;
  }
  return false;
}

/** Refetch everything on screen, once. What a reconnect does: the socket was
 *  down, every event sent during the gap went nowhere, and no amount of
 *  listening will get them back. */
export function refetchEverything(deviceId = null) {
  const woken = deviceId === null ? [...watchers] : [...watchers].filter((watcher) => watcher.hears(deviceId));
  woken.forEach(deliver);
}

/**
 * Greet a freshly live session: ask what this bridge can do, arm event mode if
 * it pushes, and read everything once.
 *
 * Every live session comes through here — the first one, a reconnect, a switch
 * to another device — and each of them has a gap behind it that announced
 * nothing, so the refetch is unconditional and so is the re-arming: the device
 * on the other end may not be the one that answered last time.
 *
 * A bridge that predates push invalidation refuses `session.hello`. That is the
 * feature detection, and it is the whole of it — the client goes back to
 * polling with nothing to configure.
 */
export async function greetBridge(call, { deviceId = null, isCurrent = () => true, onGreeting = () => {} } = {}) {
  let greeting = null;
  try {
    greeting = await call("session.hello");
  } catch {
    greeting = null; // an old bridge, or one that dropped mid-greeting
  }
  // A slower old device can answer after another session has been adopted.
  // Its features and gap belong to that old session, not the current app.
  if (!isCurrent()) return changeEventsArmed(deviceId);
  onGreeting(greeting);
  armChangeEvents(greeting, deviceId);
  refetchEverything(deviceId);
  return changeEventsArmed(deviceId);
}
