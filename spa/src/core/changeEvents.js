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

// # Subscriptions (wire spec step 1.6)
//
// A bridge that advertises `changes.subscriptions` in its greeting is told
// what each surface wants instead of pushing everything to everyone: a scope
// (one entity, the board, or every entity the board lists), the kinds that
// surface reads, the cadence it wants them at, and whether it is foreground
// work or a cache warm-up. The manager below keeps that desired map, diffs it
// against what the session already holds, and issues the subscribe and
// unsubscribe calls — on mount, on unmount, on greet, and on reconnect, where
// the new session holds nothing and the whole map is replayed.
//
// The events are then routed by entity id to the surfaces showing it. A
// surface that gave an `onChanges` receives the items and does its own key
// comparison; one that did not gets its poll callback run, exactly as a legacy
// `entity.changed` ran it. Either way the safety poll stays behind it.
//
// # The adapter (wire spec step 2.5)
//
// Every greeting also selects the API adapter for the bridge that answered it
// (`selectAdapter`, core/bridgeApi) and installs it — on the session, through
// the caller's `install`, and here for `bridgeCapabilities()`. A reconnect or
// a device switch onto another bridge version therefore re-selects, and a
// surface asks the capabilities, never the version string. A bridge no
// adapter here speaks to installs nothing and arms nothing: the gate takes
// the screen, and nothing below it is asked to guess at a shape.

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
 *  a greeting, and against a bridge nobody here speaks to. */
const NO_CAPABILITIES = Object.freeze({
  changes: Object.freeze({ subscriptions: false }),
  requests: Object.freeze({ priority: false }),
  errors: Object.freeze({ codes: false }),
});

let armed = false;
let lastApiVersion = PRE_ALPHA_API_VERSION;
/** The adapter the last greeting installed, null before one has and for a
 *  bridge no adapter claims. */
let installedAdapter = null;
const watchers = new Set();
let visibilityWired = false;

/** The session the subscriptions are held on, and whether this bridge serves
 *  them at all. Nothing is asked of a bridge that advertises none: that client
 *  keeps the legacy events and the polls it has always had. */
let sessionCall = null;
let subscriptionsMode = false;
let wantSubscriptions = false;
/** subscription_id → the spec the bridge is holding for it, serialised. */
const liveSubscriptions = new Map();
const modeListeners = new Set();
let watcherSeq = 0;
let syncChain = Promise.resolve();
let syncQueued = false;

/** Read the bridge's greeting. Returns whether event mode armed. */
export function armChangeEvents(greeting) {
  const nowArmed = Boolean(greeting && greeting.push_events === true);
  if (nowArmed === armed) return armed;
  armed = nowArmed;
  // A surface mounted before the mode was known (or across a reconnect onto a
  // different bridge) keeps polling at whatever cadence it started with unless
  // it is re-timed here.
  watchers.forEach(startTimer);
  return armed;
}

/** Whether the bridge on the other end pushes change events. */
export function changeEventsArmed() {
  return armed;
}

/** The `api_version` the last greeting reported — `0.0.0` for a bridge that
 *  reported none, or refused the greeting altogether. Diagnostics only: a
 *  surface asks `bridgeCapabilities()`, never this. */
export function bridgeApiVersion() {
  return lastApiVersion;
}

/** The adapter the last greeting installed, or null. */
export function bridgeAdapter() {
  return installedAdapter;
}

/** What the bridge on the other end can do, off the installed adapter: every
 *  flag off before a greeting and for a bridge no adapter here claims. */
export function bridgeCapabilities() {
  return installedAdapter ? installedAdapter.capabilities : NO_CAPABILITIES;
}

/** The interval a surface polling every `fastMs` should actually run at. Event
 *  mode stands a fast poll down to the safety poll and leaves a slow one alone —
 *  standing down must never mean speeding up. */
export function pollIntervalMs(fastMs) {
  return armed ? Math.max(fastMs, SAFETY_POLL_MS) : fastMs;
}

/** Forget every watcher and disarm. Tests, and a client that lost its session. */
export function resetChangeEvents() {
  watchers.forEach((watcher) => clearInterval(watcher.timer));
  watchers.clear();
  armed = false;
  lastApiVersion = PRE_ALPHA_API_VERSION;
  installedAdapter = null;
  sessionCall = null;
  wantSubscriptions = false;
  liveSubscriptions.clear();
  setSubscriptionsMode(false);
}

/** The ids a watcher stands for right now. Read at delivery, never at mount:
 *  a surface whose entity resolves late (a branch view learns its run id from
 *  the first read) must be asked about the entity it is showing now. */
function entityIdsOf(watcher) {
  const named = typeof watcher.entity === "function" ? watcher.entity() : watcher.entity;
  const list = Array.isArray(named) ? named : [named];
  return list.filter((id) => id !== null && id !== undefined && id !== "").map(String);
}


/** Whether the session on the wire speaks subscriptions. What the cache layer
 *  asks before it decides between the background tier and its 60 s loop. */
export function subscriptionsActive() {
  return subscriptionsMode;
}

/** Hear about the contract flipping — a reconnect onto an older bridge falls
 *  back to legacy, and whoever timed itself off subscriptions must re-time. */
export function onSubscriptionsChange(fn) {
  modeListeners.add(fn);
  return () => modeListeners.delete(fn);
}

function setSubscriptionsMode(active) {
  if (subscriptionsMode === active) return;
  subscriptionsMode = active;
  [...modeListeners].forEach((listener) => listener(active));
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

/** Everything the mounted surfaces want, right now. */
function desiredSubscriptions() {
  const desired = new Map();
  for (const watcher of watchers) {
    for (const spec of specsOf(watcher)) desired.set(spec.subscription_id, spec);
  }
  return desired;
}

/** One subscription call. `false` stops the diff: either the bridge refused —
 *  and the map will be replayed on the next greeting — or the session under it
 *  was replaced while this was in flight. */
async function askBridge(call, method, params) {
  try {
    await call(method, params);
  } catch {
    return false;
  }
  return sessionCall === call;
}

async function dropStale(call, desired) {
  for (const id of [...liveSubscriptions.keys()]) {
    if (desired.has(id)) continue;
    liveSubscriptions.delete(id);
    if (!(await askBridge(call, "changes.unsubscribe", { subscription_id: id }))) return false;
  }
  return true;
}

async function addDesired(call, desired) {
  for (const [id, spec] of desired) {
    const wire = JSON.stringify(spec);
    if (liveSubscriptions.get(id) === wire) continue;
    liveSubscriptions.set(id, wire);
    if (!(await askBridge(call, "changes.subscribe", spec))) {
      liveSubscriptions.delete(id);
      return false;
    }
  }
  return true;
}

/** The diff: what the bridge holds and nobody wants goes, what is wanted and
 *  not held (or held at another cadence) is upserted. A spec that has not moved
 *  costs nothing. */
async function syncSubscriptions() {
  const call = sessionCall;
  if (!subscriptionsMode || !call) return;
  const desired = desiredSubscriptions();
  if (await dropStale(call, desired)) await addDesired(call, desired);
}

/** Queue one diff behind whatever is already running. Mount, unmount, greet,
 *  every delivery and every safety tick schedule one: a surface whose entity id
 *  resolved late is subscribed on the first of those. */
function scheduleSync() {
  if (!subscriptionsMode || !sessionCall || syncQueued) return;
  syncQueued = true;
  syncChain = syncChain.then(() => {
    syncQueued = false;
    return syncSubscriptions();
  });
}

/** Resolves when nothing more is owed to the bridge. */
export async function subscriptionsSettled() {
  let waited;
  do {
    waited = syncChain;
    await waited;
  } while (waited !== syncChain);
}

function startTimer(watcher) {
  clearInterval(watcher.timer);
  watcher.timer = setInterval(() => {
    scheduleSync();
    watcher.tick();
  }, pollIntervalMs(watcher.intervalMs));
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
  catchUpOnVisible: true,
  pausesWhileHidden: true,
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
 * `board.changed` is its event.
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
    // A surface that named no entity is the feed, whatever its entity getter
    // would answer later.
    boardScoped: spec.scope !== "all" && spec.entity === null,
    missed: false,
    timer: null,
    tick: spec.pausesWhileHidden ? whenVisible(spec.refresh) : spec.refresh,
  };
  watchers.add(watcher);
  startTimer(watcher);
  scheduleSync();
  if (!visibilityWired && typeof document !== "undefined") {
    document.addEventListener("visibilitychange", onVisibilityChange);
    visibilityWired = true;
  }
  return {
    dispose() {
      clearInterval(watcher.timer);
      watcher.timer = null;
      watchers.delete(watcher);
      scheduleSync();
    },
  };
}

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

function dispatchItems(items) {
  const list = Array.isArray(items) ? items.filter(Boolean) : [];
  if (!list.length) return false;
  for (const watcher of [...watchers]) {
    const mine = itemsFor(watcher, list);
    if (mine.length) deliverChanges(watcher, mine);
  }
  scheduleSync();
  return true;
}

/** A change event off the session. Ignored entirely while unarmed — an old
 *  bridge sends none, and a client that never greeted must behave as if it
 *  could not hear them. Returns whether the event was one we act on. */
export function dispatchChangeEvent(payload) {
  if (!armed || !payload) return false;
  if (payload.type === "board.changed") {
    [...watchers].filter((watcher) => watcher.boardScoped).forEach(deliver);
    return true;
  }
  if (payload.type === "changes") return dispatchItems(payload.items);
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
export function refetchEverything() {
  [...watchers].forEach(deliver);
  scheduleSync();
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
 * way the ask rides the first greeting of every later session; a bridge that
 * does not serve it ignores the field and answers legacy, which puts the client
 * back where it started.
 */
async function negotiate(call, isCurrent) {
  const greeting = await hello(call, wantSubscriptions);
  // A slower old device can answer after another session has been adopted.
  // Its features and gap belong to that old session, not the current app.
  if (!isCurrent()) return { greeting, current: false };
  if (wantSubscriptions || !advertisesSubscriptions(greeting, call)) return { greeting, current: true };
  const asked = await hello(call, true);
  if (!isCurrent()) return { greeting, current: false };
  return { greeting: asked || greeting, current: true };
}

/** The session the desired map is now held on. It holds none of it yet — a
 *  reconnect is a new session, and a re-greet of the same one has dropped
 *  whatever it had — so the whole map is replayed. */
function adoptGreetedSession(call) {
  sessionCall = call;
  liveSubscriptions.clear();
  wantSubscriptions = bridgeCapabilities().changes.subscriptions;
  setSubscriptionsMode(wantSubscriptions);
  scheduleSync();
}

/** A bridge no adapter here claims. Nothing is armed and nothing is asked of
 *  it: the version gate owns the screen until a greeting selects an adapter. */
function abandonBridge() {
  armChangeEvents(null);
  sessionCall = null;
  liveSubscriptions.clear();
  setSubscriptionsMode(false);
  return false;
}

/**
 * @param call the session's rpc, which the greeting rides.
 * @param isCurrent whether the session greeted still owns the application
 *   when its greeting lands; a late one is dropped whole.
 * @param onGreeting hears the greeting a current session got, as sent.
 * @param install is handed what `selectAdapter` picked — `{ create }` or
 *   `{ unsupported }` — and returns the adapter now installed, or null. The
 *   session installs it on itself this way; without one the adapter is bound
 *   to `call` here.
 */
export async function greetBridge(
  call,
  {
    isCurrent = () => true,
    onGreeting = () => {},
    install = (selection) => (selection.unsupported ? null : selection.create(call)),
  } = {},
) {
  const { greeting, current } = await negotiate(call, isCurrent);
  if (!current) return changeEventsArmed();
  lastApiVersion = greetingVersion(greeting);
  installedAdapter = install(selectAdapter(greeting)) || null;
  if (!installedAdapter) return abandonBridge();
  onGreeting(greeting);
  armChangeEvents(greeting);
  adoptGreetedSession(call);
  refetchEverything();
  return changeEventsArmed();
}
