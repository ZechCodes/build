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
//
// # One bridge is not the account
//
// Every fact below belongs to ONE machine: whether it pushes, which adapter
// speaks to it, what it is subscribed to, which board revision it has been read
// for. So each of them is held per device and every entry point takes the device
// the answer is about. A watcher says which device it is about at registration
// and carries the predicate `hears(deviceId)` from then on, so nothing asks
// "did this watcher name a device?" at delivery time.
//
// # Subscriptions (wire spec step 1.6)
//
// A bridge that advertises `changes.subscriptions` in its greeting is told
// what each surface wants instead of pushing everything to everyone: a scope
// (one entity, the board, or every entity the board lists), the kinds that
// surface reads, the cadence it wants them at, and whether it is foreground
// work or a cache warm-up. The manager below keeps that desired map per device,
// diffs it against what that session already holds, and issues the subscribe and
// unsubscribe calls — on mount, on unmount, on greet, and on reconnect, where
// the new session holds nothing and the whole map is replayed. A watcher that
// spans devices is subscribed on every bridge that serves subscriptions, under
// the same `subscription_id` on each: the bridge namespaces ids per session.
//
// The events are then routed by entity id to the surfaces showing it. A
// surface that gave an `onChanges` receives the items and does its own key
// comparison; one that did not gets its poll callback run, exactly as a legacy
// `entity.changed` ran it. Either way the safety poll stays behind it.
//
// The board item carries the one key this file compares itself: `state.revision`,
// the counter the bridge bumps on every `note_board`. A flush repeating the
// revision that device's feed already read for is not delivered to the
// board-scoped surfaces — and a reconnect forgets it, because the session that
// follows a gap has to read once whatever the counter says.
//
// # The adapter (wire spec step 2.5)
//
// Every greeting also selects the API adapter for the bridge that answered it
// (`selectAdapter`, core/bridgeApi) and installs it — on that device's session,
// through the caller's `install`, and here for `bridgeCapabilities(deviceId)`.
// A reconnect onto another bridge version therefore re-selects, and a surface
// asks the capabilities, never the version string. A bridge no adapter here
// speaks to installs nothing and arms nothing: that machine answers nothing,
// and nothing below is asked to guess at a shape.

import { pageVisible, whenVisible } from "./visibility.js";
import { greetingVersion, PRE_ALPHA_API_VERSION, selectAdapter, SPA_API_RANGE } from "./bridgeApi/index.js";

/** The wire API majors this build of the SPA speaks, declared in every
 *  greeting so `bridge.stats` can count who is still on which. */
export { SPA_API_RANGE };

/** How often an armed surface still reads on its own. Events do the work; this
 *  is what catches whatever an event never covered — a bridge restart, a
 *  coalesced flush lost to a socket hiccup — without being a poll anyone waits
 *  on. */
export const SAFETY_POLL_MS = 60000;

/** What no adapter claims: every capability off. What a surface reads before
 *  a greeting, and about a bridge nobody here speaks to. */
const NO_CAPABILITIES = Object.freeze({
  changes: Object.freeze({ subscriptions: false }),
  requests: Object.freeze({ priority: false }),
  errors: Object.freeze({ codes: false }),
});

// Event mode is a fact about one bridge, so it is held per device: one machine
// pushing says nothing about another that predates push invalidation.
const armed = new Map(); // deviceId → whether that bridge pushes
const bridges = new Map(); // deviceId → what that greeted bridge holds
const watchers = new Set();
const modeListeners = new Set();
let visibilityWired = false;
let watcherSeq = 0;

/** An id as this module keys it: text, with "nothing named" reading as "". */
const idText = (value) => (value === null || value === undefined ? "" : String(value));

/** A caller that named no device at all: a surface that spans them, a reconnect
 *  that wakes everything, a test. Asked in this one place. */
const spansDevices = (deviceId) => deviceId === null || deviceId === undefined;

/** A device id as this module keys it. A caller with no device to name is the
 *  unnamed one. */
const deviceKeyOf = idText;

/** Everything one greeted bridge holds: the session it was greeted on, the
 *  adapter that speaks to it, what it is subscribed to and how far its board
 *  has been read. Nothing here is the account's. */
function newBridgeState(deviceId) {
  return {
    deviceId,
    call: null,
    adapter: null,
    apiVersion: PRE_ALPHA_API_VERSION,
    subscriptions: false, // whether this session is serving the contract
    want: false, // whether the next greeting asks for it up front
    live: new Map(), // subscription_id → the spec this bridge is holding
    boardRevision: null, // the newest revision this device's feed has read for
    chain: Promise.resolve(),
    queued: false,
  };
}

/** What this device's bridge holds, or null when it has never been greeted. */
const bridgeFor = (deviceId) => bridges.get(deviceKeyOf(deviceId)) || null;

/** The same, created on first sight — only a greeting may call this. */
function bridgeState(deviceId) {
  const device = deviceKeyOf(deviceId);
  const held = bridges.get(device) || newBridgeState(device);
  bridges.set(device, held);
  return held;
}

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

/** Retire a device: what it pushed is nobody's cadence any more, and what it
 *  was holding subscriptions for went with its session. */
export function disarmChangeEvents(deviceId) {
  const device = deviceKeyOf(deviceId);
  const state = bridges.get(device);
  if (state) {
    state.live.clear();
    setSubscriptionsMode(state, false);
    bridges.delete(device);
  }
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
  if (spansDevices(deviceId)) return armed.size > 0 && [...armed.values()].every(Boolean);
  return armedFor(deviceId);
}

/** One bridge's mode, with no spanning rule over it: what an event arriving
 *  from that device is measured against. */
const armedFor = (deviceId) => armed.get(deviceKeyOf(deviceId)) === true;

/** The `api_version` this device's last greeting reported — `0.0.0` for a bridge
 *  that reported none, refused the greeting, or has never been greeted.
 *  Diagnostics only: a surface asks `bridgeCapabilities(deviceId)`. */
export function bridgeApiVersion(deviceId = null) {
  return bridgeFor(deviceId)?.apiVersion || PRE_ALPHA_API_VERSION;
}

/** The adapter this device's last greeting installed, or null. */
export function bridgeAdapter(deviceId = null) {
  return bridgeFor(deviceId)?.adapter || null;
}

/** What this device's bridge can do, off its installed adapter: every flag off
 *  before a greeting, for an unknown device, and for a bridge no adapter here
 *  claims. */
export function bridgeCapabilities(deviceId = null) {
  return bridgeFor(deviceId)?.adapter?.capabilities || NO_CAPABILITIES;
}

/** The interval a surface polling every `fastMs` should actually run at. Event
 *  mode stands a fast poll down to the safety poll and leaves a slow one alone —
 *  standing down must never mean speeding up. */
export function pollIntervalMs(fastMs, deviceId = null) {
  return changeEventsArmed(deviceId) ? Math.max(fastMs, SAFETY_POLL_MS) : fastMs;
}

/** Forget every watcher and every bridge. Tests, and a client that lost its
 *  sessions. */
export function resetChangeEvents() {
  watchers.forEach((watcher) => clearInterval(watcher.timer));
  watchers.clear();
  armed.clear();
  for (const state of [...bridges.values()]) {
    state.live.clear();
    setSubscriptionsMode(state, false);
  }
  bridges.clear();
}

/** The ids a watcher stands for right now. Read at delivery, never at mount:
 *  a surface whose entity resolves late (a branch view learns its run id from
 *  the first read) must be asked about the entity it is showing now. */
function entityIdsOf(watcher) {
  const named = typeof watcher.entity === "function" ? watcher.entity() : watcher.entity;
  const list = Array.isArray(named) ? named : [named];
  return list.map(idText).filter(Boolean);
}

/** The predicate a watcher is registered with: the whole of "does this surface
 *  hear that device?", asked once per delivery and never re-derived. */
const hearsFor = (deviceId) =>
  spansDevices(deviceId) ? () => true : (device) => deviceKeyOf(device) === deviceKeyOf(deviceId);

/** Whether this device's session speaks subscriptions. What the cache layer asks
 *  before it decides between the background tier and its 60 s loop. */
export function subscriptionsActive(deviceId = null) {
  return bridgeFor(deviceId)?.subscriptions === true;
}

/** Hear about a device's contract flipping — a reconnect onto an older bridge
 *  falls back to legacy, and whoever timed itself off subscriptions must
 *  re-time. Called with `(deviceId, active)`. */
export function onSubscriptionsChange(fn) {
  modeListeners.add(fn);
  return () => modeListeners.delete(fn);
}

function setSubscriptionsMode(state, active) {
  if (state.subscriptions === active) return;
  state.subscriptions = active;
  [...modeListeners].forEach((listener) => listener(state.deviceId, active));
}

/** One subscription per scope a watcher stands for: an entity-scoped surface
 *  showing two ids wants two, and the ids are read now, not at mount. */
function scopesOf(watcher) {
  if (watcher.scope === "all") return [{ id: watcher.id, scope: { kind: "all" } }];
  if (watcher.boardScoped) return [{ id: watcher.id, scope: { kind: "board" } }];
  return entityIdsOf(watcher).map((entityId) => ({
    id: `${watcher.id}:${entityId}`,
    scope: { kind: "entity", id: entityId },
  }));
}

/** Board scope carries feed-level state only. A surface that watches the board
 *  and reads git (the primary checkout's pane) is trimmed here rather than
 *  refused there. */
const kindsFor = (watcher, scope) =>
  scope.kind === "board" ? watcher.kinds.filter((kind) => kind === "state") : watcher.kinds;

function specsOf(watcher) {
  if (!watcher.kinds.length) return [];
  return scopesOf(watcher).flatMap(({ id, scope }) => {
    const kinds = kindsFor(watcher, scope);
    if (!kinds.length) return [];
    return [{ subscription_id: id, scope, kinds, mode: watcher.mode, priority: watcher.priority }];
  });
}

/** Everything the surfaces listening to this device want, right now. */
function desiredSubscriptions(deviceId) {
  const desired = new Map();
  for (const watcher of watchers) {
    if (!watcher.hears(deviceId)) continue;
    for (const spec of specsOf(watcher)) desired.set(spec.subscription_id, spec);
  }
  return desired;
}

/** One subscription call. `false` stops the diff: either the bridge refused —
 *  and the map will be replayed on the next greeting — or the session under it
 *  was replaced while this was in flight. */
async function askBridge(state, method, params) {
  const call = state.call;
  try {
    await call(method, params);
  } catch {
    return false;
  }
  return state.call === call;
}

async function dropStale(state, desired) {
  for (const id of [...state.live.keys()]) {
    if (desired.has(id)) continue;
    state.live.delete(id);
    if (!(await askBridge(state, "changes.unsubscribe", { subscription_id: id }))) return false;
  }
  return true;
}

async function addDesired(state, desired) {
  for (const [id, spec] of desired) {
    const wire = JSON.stringify(spec);
    if (state.live.get(id) === wire) continue;
    state.live.set(id, wire);
    if (!(await askBridge(state, "changes.subscribe", spec))) {
      state.live.delete(id);
      return false;
    }
  }
  return true;
}

/** The diff for one device: what its bridge holds and nobody wants goes, what is
 *  wanted and not held (or held at another cadence) is upserted. A spec that has
 *  not moved costs nothing. */
async function syncSubscriptions(state) {
  if (!state.subscriptions || !state.call) return;
  const desired = desiredSubscriptions(state.deviceId);
  if (await dropStale(state, desired)) await addDesired(state, desired);
}

/** Queue one diff for one device behind whatever is already running for it.
 *  Mount, unmount, greet, every delivery and every safety tick schedule one: a
 *  surface whose entity id resolved late is subscribed on the first of those. */
function scheduleSync(deviceId) {
  const state = bridgeFor(deviceId);
  if (!state || !state.subscriptions || !state.call || state.queued) return;
  state.queued = true;
  state.chain = state.chain.then(() => {
    state.queued = false;
    return syncSubscriptions(state);
  });
}

/** Every bridge this watcher is heard by owes a diff: a watcher naming a device
 *  moves that one, and one that spans devices moves them all. */
function scheduleSyncHeardBy(watcher) {
  for (const state of [...bridges.values()]) {
    if (watcher.hears(state.deviceId)) scheduleSync(state.deviceId);
  }
}

const pendingChains = () => [...bridges.values()].map((state) => state.chain);
const sameChains = (before, after) => before.length === after.length && before.every((chain, index) => chain === after[index]);

/** Resolves when nothing more is owed to any bridge. */
export async function subscriptionsSettled() {
  let before;
  do {
    before = pendingChains();
    await Promise.all(before);
  } while (!sameChains(before, pendingChains()));
}

function startTimer(watcher) {
  clearInterval(watcher.timer);
  // `keepPolling` holds a watcher at its own cadence instead of standing it
  // down to the safety poll once a pushing bridge is armed.
  const interval = watcher.keepPolling ? watcher.intervalMs : pollIntervalMs(watcher.intervalMs, watcher.deviceId);
  watcher.timer = setInterval(() => {
    scheduleSyncHeardBy(watcher);
    watcher.tick();
  }, interval);
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

/** What a registration leaves unsaid. Held here rather than as destructuring
 *  defaults so `watchChanges` stays inside the complexity gate. */
const WATCHER_DEFAULTS = {
  entity: null,
  deviceId: null,
  catchUpOnVisible: true,
  pausesWhileHidden: true,
  keepPolling: false,
  kinds: [],
  mode: "realtime",
  priority: "foreground",
  scope: null,
  onChanges: null,
};

function withDefaults(registration, defaults) {
  const spec = { ...defaults };
  for (const [key, value] of Object.entries(registration)) {
    if (value !== undefined) spec[key] = value;
  }
  return spec;
}

/**
 * Register a polling surface.
 *
 * `refresh` is the surface's existing poll callback — the one whose paint the
 * whole surface is built around. `intervalMs` is the cadence it polls at while
 * nothing is pushing; event mode re-times it to the safety poll. `entity` names
 * what the surface is showing (a string, or a function returning a string or a
 * list); leaving it out makes the surface board-scoped — it is the feed, and
 * `board.changed` is its event. `deviceId` names the machine the surface is
 * about; leaving it out makes the surface one that spans every device.
 *
 * `kinds` names what the surface reads — any of `state`, `thread`, `git`,
 * `files` — and is what the bridge subscribes it to; a surface that names none
 * asks for no subscription and lives on the legacy events and its poll. `mode`
 * is `"realtime"` for a mounted surface or `{ batch_ms: N }` for a background
 * tier, `priority` orders the flush and the pulls it causes, and `scope: "all"`
 * is the background tier's whole-board watch. `onChanges(items)` receives the
 * items for the entities this surface stands for; without one, an item runs
 * `refresh` instead, which is what a legacy event always did.
 *
 * `keepPolling: true` holds the surface at its own `intervalMs` even once an
 * event-pushing bridge is armed, for a scope the bridge does not push for.
 *
 * `catchUpOnVisible: false` is for a surface that already refreshes itself on
 * visibilitychange (the feed does), so coming back does not read twice.
 * `pausesWhileHidden: false` is for a surface whose poll is not visibility-gated
 * — its events are not gated either, because parity with the poll is the rule.
 *
 * Returns `{ dispose }`; it owns the interval, so the caller stops clearing one.
 */
export function watchChanges(registration) {
  const spec = withDefaults(registration, WATCHER_DEFAULTS);
  const watcher = {
    id: `sub-${++watcherSeq}`,
    ...spec,
    // Whose events this surface is about, decided once, here: a surface that
    // named a device hears that device, and one that named none spans them all.
    hears: hearsFor(spec.deviceId),
    // A surface that named no entity is the feed, whatever its entity getter
    // would answer later.
    boardScoped: spec.scope !== "all" && spec.entity === null,
    missed: false,
    timer: null,
    tick: spec.pausesWhileHidden ? whenVisible(spec.refresh) : spec.refresh,
  };
  watchers.add(watcher);
  startTimer(watcher);
  scheduleSyncHeardBy(watcher);
  if (!visibilityWired && typeof document !== "undefined") {
    document.addEventListener("visibilitychange", onVisibilityChange);
    visibilityWired = true;
  }
  return {
    dispose() {
      clearInterval(watcher.timer);
      watcher.timer = null;
      watchers.delete(watcher);
      scheduleSyncHeardBy(watcher);
    },
  };
}

const watchersWhere = (matches) => [...watchers].filter(matches);

/** The items of one flush that belong to this watcher: the board item for the
 *  feed, every entity item for the background tier, and the ones naming an id
 *  a surface is showing for everyone else. */
function itemsFor(watcher, items) {
  if (watcher.scope === "all") return items.filter((item) => String(item.entity_id) !== "board");
  if (watcher.boardScoped) return items.filter((item) => String(item.entity_id) === "board");
  const ids = entityIdsOf(watcher);
  return items.filter((item) => ids.includes(String(item.entity_id)));
}

/** Hand a watcher what moved. A surface with no `onChanges` has no key to
 *  compare, so the item is the same news `entity.changed` was: refetch. */
function deliverChanges(watcher, items) {
  if (!watcher.onChanges) {
    deliver(watcher);
    return;
  }
  if (watcher.pausesWhileHidden && !pageVisible()) {
    watcher.missed = true;
    return;
  }
  watcher.missed = false;
  watcher.onChanges(items);
}

/** The revision the board item of this flush carries, or null for a flush
 *  that carries no board item — or one from a bridge that names no revision. */
function boardRevisionOf(items) {
  const board = items.find((item) => String(item.entity_id) === "board");
  const revision = board && board.state ? board.state.revision : undefined;
  return typeof revision === "number" ? revision : null;
}

/** Whether this flush's board item is news to that device's feed. The bridge
 *  bumps the revision on every `note_board`, so an item repeating the one
 *  already seen is a flush the feed has nothing to read for. A board item that
 *  names no revision always is news: that is a bridge from before the counter,
 *  and guessing on its behalf would drop a real change. */
function boardItemIsNews(items, state) {
  const revision = boardRevisionOf(items);
  if (revision === null || !state) return true;
  if (state.boardRevision !== null && revision <= state.boardRevision) return false;
  state.boardRevision = revision;
  return true;
}

function dispatchItems(items, deviceId) {
  const list = Array.isArray(items) ? items.filter(Boolean) : [];
  if (!list.length) return false;
  const boardIsNews = boardItemIsNews(list, bridgeFor(deviceId));
  for (const watcher of [...watchers]) {
    if (!watcher.hears(deviceId)) continue;
    const mine = itemsFor(watcher, list);
    if (!mine.length || (watcher.boardScoped && !boardIsNews)) continue;
    deliverChanges(watcher, mine);
  }
  scheduleSync(deviceId);
  return true;
}

const wake = (audience) => {
  audience.forEach(deliver);
  return true;
};

/** Who each kind of event wakes. The board moved on a device, so its board
 *  watchers read again; an entity moved, so whoever is showing it does —
 *  wherever it is, since an entity id is the same id on any surface holding it;
 *  a subscription flush is routed item by item. A kind with no entry here is an
 *  event this client does not act on. */
const EVENT_DISPATCHERS = new Map([
  ["board.changed", (payload, deviceId) => wake(watchersWhere((watcher) => watcher.boardScoped && watcher.hears(deviceId)))],
  [
    "entity.changed",
    (payload) => {
      const id = idText(payload.id);
      return id ? wake(watchersWhere((watcher) => entityIdsOf(watcher).includes(id))) : false;
    },
  ],
  ["changes", (payload, deviceId) => dispatchItems(payload.items, deviceId)],
]);

/** A change event off one device's session. Ignored entirely while that device
 *  is unarmed — an old bridge sends none, and a client that never greeted must
 *  behave as if it could not hear them. Returns whether the event was one we act
 *  on. */
export function dispatchChangeEvent(payload, deviceId = null) {
  if (!payload || !armedFor(deviceId)) return false;
  const dispatch = EVENT_DISPATCHERS.get(payload.type);
  return dispatch ? dispatch(payload, deviceId) : false;
}

/** Refetch everything on screen that hears this device, once. What a reconnect
 *  does: the socket was down, every event sent during the gap went nowhere, and
 *  no amount of listening will get them back. */
export function refetchEverything(deviceId = null) {
  const woken = spansDevices(deviceId) ? [...watchers] : watchersWhere((watcher) => watcher.hears(deviceId));
  woken.forEach(deliver);
  woken.forEach((watcher) => scheduleSyncHeardBy(watcher));
}

/** Who is greeting: this SPA, the build it was stamped as (the git SHA in a
 *  CI build, `dev` otherwise — the same value the version watcher compares),
 *  and the API range it speaks. */
function clientDeclaration() {
  return {
    name: "spa",
    version: import.meta.env.VITE_BUILD_VERSION || "dev",
    api_range: SPA_API_RANGE,
  };
}

/** What the adapter this greeting selects says the bridge can do — read off
 *  the greeting through the adapter, never probed for. Nothing is installed
 *  here: the greeting that ends the negotiation is the one that installs. */
function capabilitiesFor(greeting, call) {
  const selection = selectAdapter(greeting);
  return selection.unsupported ? NO_CAPABILITIES : selection.create(call).capabilities;
}

/** Whether this bridge serves subscriptions. A bridge that predates them
 *  says nothing here and is left on the legacy events. */
const advertisesSubscriptions = (greeting, call) => capabilitiesFor(greeting, call).changes.subscriptions === true;

async function hello(call, subscriptions) {
  try {
    return await call("session.hello", {
      client: clientDeclaration(),
      ...(subscriptions ? { changes: "subscriptions" } : {}),
    });
  } catch {
    return null; // an old bridge, or one that dropped mid-greeting
  }
}

/**
 * Greet, and ask for the subscriptions contract only where the greeting
 * advertises it. The first greeting to an unknown bridge is a legacy one — the
 * contract cannot be asked for before the bridge has said it serves it — and a
 * second greeting switches the session over. Once a bridge has answered that
 * way the ask rides the first greeting of every later session with it; a bridge
 * that does not serve it ignores the field and answers legacy, which puts the
 * client back where it started.
 */
async function negotiate(call, deviceId, isCurrent) {
  const want = bridgeFor(deviceId)?.want === true;
  const greeting = await hello(call, want);
  // A slower old device can answer after its session has been replaced. Its
  // features and gap belong to that old session, not to this device now.
  if (!isCurrent()) return { greeting, current: false };
  if (want || !advertisesSubscriptions(greeting, call)) return { greeting, current: true };
  const asked = await hello(call, true);
  if (!isCurrent()) return { greeting, current: false };
  return { greeting: asked || greeting, current: true };
}

/** The session this device's desired map is now held on. It holds none of it
 *  yet — a reconnect is a new session, and a re-greet of the same one has
 *  dropped whatever it had — so the whole map is replayed. */
function adoptGreetedSession(state, call) {
  state.call = call;
  // The gap behind this session announced nothing and the bridge answering it
  // may not be the one that counted to the revision last seen, so the first
  // board item of the new session always refreshes.
  state.boardRevision = null;
  state.live.clear();
  state.want = bridgeCapabilities(state.deviceId).changes.subscriptions;
  setSubscriptionsMode(state, state.want);
  scheduleSync(state.deviceId);
}

/** A bridge no adapter here claims. Nothing is armed and nothing is asked of
 *  it: that machine answers nothing until a greeting selects an adapter. */
function abandonBridge(state) {
  armChangeEvents(null, state.deviceId);
  state.call = null;
  state.live.clear();
  setSubscriptionsMode(state, false);
  return false;
}

/**
 * Greet a freshly live session: ask what that bridge can do, arm event mode if
 * it pushes, and read everything about it once.
 *
 * Every live session comes through here — an account's first, a reconnect, a
 * device joining the ones already open — and each of them has a gap behind it
 * that announced nothing, so the refetch is unconditional. So is the re-arming
 * and the re-selection: both are facts about one bridge, and a bridge that has
 * just been restarted or updated may not do what it did the last time.
 *
 * A bridge that predates push invalidation refuses `session.hello`. That is the
 * feature detection, and it is the whole of it — the client goes back to
 * polling with nothing to configure.
 *
 * @param call the session's rpc, which the greeting rides.
 * @param deviceId the machine this session is on; every fact the greeting
 *   settles is kept under it.
 * @param isCurrent whether the session greeted still owns this device when its
 *   greeting lands; a late one is dropped whole.
 * @param onGreeting hears the greeting a current session got, as sent.
 * @param install is handed what `selectAdapter` picked — `{ create }` or
 *   `{ unsupported }` — and returns the adapter now installed, or null. The
 *   session installs it on itself this way; without one the adapter is bound
 *   to `call` here.
 */
export async function greetBridge(
  call,
  {
    deviceId = null,
    isCurrent = () => true,
    onGreeting = () => {},
    install = (selection) => (selection.unsupported ? null : selection.create(call)),
  } = {},
) {
  const { greeting, current } = await negotiate(call, deviceId, isCurrent);
  if (!current) return changeEventsArmed(deviceId);
  const state = bridgeState(deviceId);
  state.apiVersion = greetingVersion(greeting);
  state.adapter = install(selectAdapter(greeting)) || null;
  if (!state.adapter) return abandonBridge(state);
  onGreeting(greeting);
  armChangeEvents(greeting, deviceId);
  adoptGreetedSession(state, call);
  refetchEverything(deviceId);
  return changeEventsArmed(deviceId);
}
