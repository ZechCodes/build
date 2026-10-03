// The subscriptions this client holds, and the pushes that arrive on them.
//
// NOTHING HERE RUNS ON A TIMER. Every detail surface used to poll on a 1.6–2 s
// interval because nothing told it anything had happened; a bridge that pushes
// tells it, and the client's whole cadence is now the flush arriving. A
// registration that names a poll is refused rather than quietly honoured
// (`watchChanges` below).
//
// What arrives is a subscription flush: a list of items, each naming an entity
// and carrying the bodies of the kinds that subscription asked for. The sync
// layer writes those bodies into the cache, and the views repaint off the
// cache (core/cacheSync.js, core/localCache.js).
//
// # Arming, and the old bridge
//
// Event mode arms only on `push_events: true` in the `session.hello` greeting.
// A bridge that predates push invalidation refuses that method, the greeting
// yields nothing, and this client hears nothing from that machine — which is
// what the ordered pass on every greeting and every reconnect is for.
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
// comparison; one that did not gets its refresh callback run. Either way the
// safety poll stays behind it.
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

import { pageVisible } from "./visibility.js";
import { recordConnectionDiagnostic } from "./connectionDiagnostics.js";
import { greetingVersion, PRE_ALPHA_API_VERSION, selectAdapter, SPA_API_RANGE } from "./bridgeApi/index.js";
import { rememberBridgeUpdateStatus } from "./bridgeUpdates.js";
import { modelsChangedOn } from "./modelCatalog.js";
import { rememberNeedsYouRule } from "./needsYouRule.js";
import { rememberBranchDelete } from "./branchDeleteSupport.js";
import { NO_REVIEW_SUPPORT, rememberReviewSupport } from "./taskReviewSupport.js";
import { rememberConversationResetSupport } from "./conversationReset.js";
import { rememberAgentLineageSupport } from "./agentLineageSupport.js";
import { rememberTaskChecklistSupport } from "./taskChecklistSupport.js";
import { rememberSourceEditSupport } from "./sourceEditSupport.js";
import { rememberProjectFilesSupport } from "./projectFilesSupport.js";
import { rememberWorkspaceSizeSupport } from "./workspaceSizeSupport.js";

/** The wire API majors this build of the SPA speaks, declared in every
 *  greeting so `bridge.stats` can count who is still on which. */
export { SPA_API_RANGE };

/** What no adapter claims: every capability off. What a surface reads before
 *  a greeting, and about a bridge nobody here speaks to. */
const NO_CAPABILITIES = Object.freeze({
  changes: Object.freeze({ subscriptions: false, kinds: Object.freeze([]) }),
  fs: Object.freeze({ projectSources: false }),
  requests: Object.freeze({ priority: false }),
  errors: Object.freeze({ codes: false }),
  diffs: Object.freeze({ perFile: false }),
  bodies: Object.freeze({ pages: false, mediaRawPages: false }),
  reviews: NO_REVIEW_SUPPORT,
  tasks: Object.freeze({
    attachments: false, watching: false, context: false, doneSinceLeft: false, commentUserNotifies: false,
    listPaged: false, bodyPrecondition: false,
  }),
  conversations: Object.freeze({ settings: false, reset: false }),
  github: Object.freeze({ repos: false }),
  messages: Object.freeze({ context: false }),
  threads: Object.freeze({ postOperations: false, attachmentChunks: false }),
  branches: Object.freeze({ finishDelete: false }),
  push: Object.freeze({ registerKey: false, revokeKey: false }),
  agents: Object.freeze({ createdBy: false }),
  projects: Object.freeze({ updateSource: false, syncBase: false }),
  workspaces: Object.freeze({ measureSizes: false }),
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
  armed.delete(device);
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
 *
 *  Diagnostics and version GATES only — a surface asking "can this bridge do X"
 *  asks `bridgeCapabilities(deviceId)`, which interprets the greeting names and
 *  legacy versions in one place (`bridgeApi/v1` `capabilitiesOf`). */
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

/** Forget every watcher and every bridge. Tests, and a client that lost its
 *  sessions. */
export function resetChangeEvents() {
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

/** Hear about a device's contract flipping — a reconnect onto a bridge that
 *  serves no subscriptions drops them, and whoever timed itself off
 *  subscriptions must re-time. Called with `(deviceId, active)`. */
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
 *  and reads git as well is trimmed here rather than refused there. */
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

/// How one subscription call ended, which is three things and not two.
///
/// DONE and REFUSED are both answers from the session that asked: the bridge
/// took the spec, or it would not. STALE is not an answer at all — the session
/// was replaced while the call was in flight, so whatever came back is about a
/// bridge this device is no longer on.
const CALL_DONE = "done";
const CALL_REFUSED = "refused";
const CALL_STALE = "stale";

/// The code a refusal named, under whichever name it arrived under: the wire's
/// `error_code` from a 1.1+ bridge, `code` once an adapter has made it an
/// ApiError, and "unknown" for a refusal that named none.
const refusalCode = (thrown) => thrown?.code || thrown?.error_code || "unknown";

/// A refused subscribe, on the record.
///
/// This is the failure that is invisible from the inside: the client asks, the
/// bridge says no, and the page simply never hears anything again. It goes in
/// the connection diagnostic history (core/connectionDiagnostics.js) so the
/// next one is readable in Settings → Diagnostics rather than only in a console
/// nobody has open — which on the phone where this was reported is no console
/// at all. Enumerated words only: the subscription's own id and the bridge's
/// code, both from closed sets.
function noteRefusedSubscription(state, method, params, thrown) {
  if (method !== "changes.subscribe") return;
  recordConnectionDiagnostic(`${state.deviceId}:subscriptions`, "subscription", {
    state: "refused",
    subscription: params?.subscription_id || "",
    code: refusalCode(thrown),
  });
}

/** One subscription call: which of the three ways it ended, and the refusal
 *  when it was one. */
async function askBridge(state, method, params) {
  const call = state.call;
  try {
    await call(method, params);
  } catch (thrown) {
    if (state.call !== call) return { outcome: CALL_STALE };
    noteRefusedSubscription(state, method, params, thrown);
    return { outcome: CALL_REFUSED, refusal: thrown };
  }
  return { outcome: state.call === call ? CALL_DONE : CALL_STALE };
}

/// The kinds a refused subscribe named as unknown to its bridge
/// (`details.kinds`, announced as `changes.refusedKinds`). An older bridge
/// names none, and its refusal stays the whole spec's.
function refusedKinds(refusal) {
  const kinds = refusal?.details?.kinds;
  return Array.isArray(kinds) ? kinds.filter((kind) => typeof kind === "string") : [];
}

/// The spec a refusal leaves worth asking for: every kind it asked for but the
/// ones the bridge named. Null when the refusal named none of them, or all of
/// them — then there is nothing different to ask for.
function withoutRefusedKinds(spec, refusal) {
  const refused = refusedKinds(refusal);
  const kinds = spec.kinds.filter((kind) => !refused.includes(kind));
  return kinds.length && kinds.length < spec.kinds.length ? { ...spec, kinds } : null;
}

/// Subscribe one spec. A bridge that refuses it for kinds it does not know says
/// which, so the spec is asked for once more without them and every other kind
/// is still delivered — one unknown kind no longer costs the surface its push.
/// Once, not in a loop: a second refusal is the spec's, as any refusal is.
async function subscribeSpec(state, spec) {
  const asked = await askBridge(state, "changes.subscribe", spec);
  const retry = asked.outcome === CALL_REFUSED ? withoutRefusedKinds(spec, asked.refusal) : null;
  return retry ? (await askBridge(state, "changes.subscribe", retry)).outcome : asked.outcome;
}

/// Whether the diff may go on after this call.
///
/// A REFUSED spec is ONE spec's problem. It used to end the whole diff, and
/// that made a single unknown field catastrophic: `s-inbox` naming a kind the
/// bridge had never heard of was refused, the loop returned, and `s-background`
/// and `s-active` were never taken out either — a device holding no
/// subscriptions at all, which hears nothing and looks exactly like a dead
/// connection. The refused id is dropped from `live` so a later pass asks
/// again (a bridge that is upgraded under this tab heals without a reload),
/// and the specs after it are still worth asking for.
///
/// STALE does end it: every spec after this one would be sent to a session
/// that has gone, and `adoptGreetedSession` replays the whole map on the new
/// one anyway.
function keepGoingAfter(state, id, outcome) {
  if (outcome === CALL_DONE) return true;
  state.live.delete(id);
  return outcome === CALL_REFUSED;
}

async function dropStale(state, desired) {
  for (const id of [...state.live.keys()]) {
    if (desired.has(id)) continue;
    state.live.delete(id);
    const { outcome } = await askBridge(state, "changes.unsubscribe", { subscription_id: id });
    if (outcome === CALL_STALE) return false;
  }
  return true;
}

async function addDesired(state, desired) {
  for (const [id, spec] of desired) {
    const wire = JSON.stringify(spec);
    if (state.live.get(id) === wire) continue;
    // The spec as wanted, even when the bridge took it without a kind it did
    // not know: the next diff compares against this and asks nothing again.
    state.live.set(id, wire);
    const outcome = await subscribeSpec(state, spec);
    if (outcome === CALL_DONE) announceHeld(state.deviceId, id);
    if (!keepGoingAfter(state, id, outcome)) return false;
  }
  return true;
}

const heldListeners = new Set();

/** Hear every subscription a bridge has just taken on — a subscribe or an
 *  upsert it answered. It records changes from that answer on and not before,
 *  so a read asked earlier than it can have missed some (core/cacheSync.js).
 *  Answers the way to stop hearing. */
export function onSubscriptionHeld(listener) {
  heldListeners.add(listener);
  return () => heldListeners.delete(listener);
}

function announceHeld(deviceId, subscriptionId) {
  for (const listener of [...heldListeners]) {
    try {
      listener(deviceId, subscriptionId);
    } catch (error) {
      console.warn("a subscription listener threw:", error);
    }
  }
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

/** Resolves when one device's bridge has answered everything asked of it so
 *  far — what a pass with something cached waits on before it reads, so a
 *  change after the read is one the bridge pushes (#142). At once for a device
 *  that was never greeted. */
export async function subscriptionsSettledFor(deviceId) {
  const state = bridgeFor(deviceId);
  let settled = null;
  while (state && settled !== state.chain) {
    settled = state.chain;
    await settled;
  }
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

/** Coming back to a tab that was told about changes it could not act on while
 *  it was away. */
function onVisibilityChange() {
  if (!pageVisible()) return;
  [...watchers].forEach((watcher) => {
    if (!watcher.missed) return;
    watcher.missed = false;
    watcher.refresh();
  });
}

/** What a registration leaves unsaid. Held here rather than as destructuring
 *  defaults so `watchChanges` stays inside the complexity gate. */
const WATCHER_DEFAULTS = {
  entity: null,
  deviceId: null,
  pausesWhileHidden: true,
  kinds: [],
  mode: "realtime",
  priority: "foreground",
  scope: null,
  onChanges: null,
};

/** What a registration may no longer say. A surface naming one of these is a
 *  surface with a poll in it, and the point of this module now is that there
 *  are none: honouring them quietly would put one back without anybody
 *  reading a line of this file. */
const RETIRED_OPTIONS = ["intervalMs", "keepPolling", "catchUpOnVisible"];

function refuseRetiredOptions(registration) {
  const named = RETIRED_OPTIONS.filter((option) => option in registration);
  if (named.length) throw new TypeError(`watchChanges does not poll: remove ${named.join(", ")}`);
}

function withDefaults(registration, defaults) {
  const spec = { ...defaults };
  for (const [key, value] of Object.entries(registration)) {
    if (value !== undefined) spec[key] = value;
  }
  return spec;
}

/**
 * Subscribe a surface to what its machine pushes.
 *
 * `refresh` is what the surface does when it hears that something it is
 * showing moved. `entity` names what that is (a string, or a function
 * returning a string or a list); leaving it out makes the surface
 * board-scoped — it is the feed. `deviceId` names the machine the surface is
 * about; leaving it out makes the surface one that spans every device.
 *
 * `kinds` names what the surface reads — any of `state`, `thread`, `git`,
 * `files`, `terminals` — and is what the bridge subscribes it to; a surface
 * that names none asks for no subscription and hears whatever arrives. `mode`
 * is `"realtime"` for a mounted surface or `{ batch_ms: N }` for a background
 * tier, `priority` orders the flush and the pulls it causes, and `scope: "all"`
 * is the background tier's whole-board watch. `onChanges(items)` receives the
 * items for the entities this surface stands for; without one, an item runs
 * `refresh` instead.
 *
 * `pausesWhileHidden: false` is for a surface that keeps up while the tab is
 * away; by default a hidden tab notes what it missed and acts on it when the
 * reader comes back.
 *
 * There is no cadence to name: a registration that names one is refused.
 *
 * Returns `{ dispose }`.
 */
export function watchChanges(registration) {
  refuseRetiredOptions(registration);
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
  };
  watchers.add(watcher);
  scheduleSyncHeardBy(watcher);
  if (!visibilityWired && typeof document !== "undefined") {
    document.addEventListener("visibilitychange", onVisibilityChange);
    visibilityWired = true;
  }
  return {
    dispose() {
      watchers.delete(watcher);
      scheduleSyncHeardBy(watcher);
    },
  };
}

const watchersWhere = (matches) => [...watchers].filter(matches);

/** The entity id the feed's own item rides under (bridge `BOARD_ITEM_ID`). */
const BOARD_ITEM_ID = "board";

const isBoardItem = (item) => String(item.entity_id) === BOARD_ITEM_ID;

/** Whether the board's own item is this watcher's. The board item is a `state`
 *  item — which entities left the feed and the lists that moved — so the
 *  bridge sends it to a whole-board subscription that asked for state and to
 *  no other, and the routing here says the same. A background tier watching
 *  git and files over the whole board is not the feed and gets none of it. */
const wantsBoardItem = (watcher) => watcher.kinds.includes("state");

/** The item fields that carry a kind's body. Everything else on an item is
 *  the envelope the bridge addressed it with. */
const KIND_FIELDS = ["state", "thread", "git", "files", "terminals"];

/** The items of one flush whose entity this watcher stands for: the board item
 *  for the feed and for the whole-board watcher that reads state, every entity
 *  item for a whole-board watcher, and the ones naming an id a surface is
 *  showing for everyone else. */
function coveredBy(watcher, items) {
  if (watcher.scope === "all") {
    return wantsBoardItem(watcher) ? items : items.filter((item) => !isBoardItem(item));
  }
  if (watcher.boardScoped) return items.filter(isBoardItem);
  const ids = entityIdsOf(watcher);
  return items.filter((item) => ids.includes(String(item.entity_id)));
}

/**
 * One item narrowed to the kinds this watcher subscribed to, or null when it
 * carries none of them.
 *
 * The bridge sends a subscription only the kinds it asked for
 * (`Subscription::wants` is scope AND kind), so this is the half of that rule
 * the client owes: a whole-board git watcher covers every entity, and handing
 * it a `state` body would have it apply a row it never asked to hear about.
 *
 * A watcher that named no kinds asked the bridge for nothing and hears items
 * whole; an item carrying no kind at all is a bare "something moved", and is
 * handed on as it arrived.
 */
function forKinds(watcher, item) {
  if (!watcher.kinds.length) return item;
  const carried = KIND_FIELDS.filter((kind) => kind in item);
  const mine = carried.filter((kind) => watcher.kinds.includes(kind));
  if (!carried.length || mine.length === carried.length) return item;
  if (!mine.length) return null;
  const narrowed = { ...item };
  for (const kind of carried) {
    if (!mine.includes(kind)) delete narrowed[kind];
  }
  return narrowed;
}

/** What this watcher is handed out of one flush: the items it stands for, each
 *  cut down to the kinds it reads. */
const itemsFor = (watcher, items) =>
  coveredBy(watcher, items)
    .map((item) => forKinds(watcher, item))
    .filter(Boolean);

/** Hand a watcher what moved. A surface with no `onChanges` has no key to
 *  compare, so the item only says something moved: refetch. */
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
  const board = items.find(isBoardItem);
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

/** The ids a watcher's subscriptions are held under on the bridge — the ones
 *  `specsOf` asks for, so a frame naming one is naming this watcher. */
const subscriptionIdsOf = (watcher) => specsOf(watcher).map((spec) => spec.subscription_id);

/**
 * Who one flush is for.
 *
 * The bridge flushes per subscription and names which one, so the frame is
 * that subscription's and nobody else's. A device holds three at once and all
 * three cover the routed workspace — handing one flush to everyone it covers
 * applies it three times and pulls three times what it should.
 *
 * Two watchers still hear a frame that names another: one holding no
 * subscription of its own (a surface that named no kinds, which asked the
 * bridge for nothing and lives on whatever arrives), and — for a frame naming
 * a subscription this client does not hold, which is a bridge from before the
 * id or one replaying a session that is gone — everyone the items cover, as
 * it always was.
 */
function audienceFor(subscriptionId, deviceId) {
  const heard = [...watchers].filter((watcher) => watcher.hears(deviceId));
  const named = idText(subscriptionId);
  if (!named) return heard;
  const addressed = [];
  const unsubscribed = [];
  for (const watcher of heard) {
    const ids = subscriptionIdsOf(watcher);
    if (!ids.length) unsubscribed.push(watcher);
    else if (ids.includes(named)) addressed.push(watcher);
  }
  return addressed.length ? [...addressed, ...unsubscribed] : heard;
}

function dispatchItems(payload, deviceId) {
  const list = Array.isArray(payload.items) ? payload.items.filter(Boolean) : [];
  if (!list.length) return false;
  const boardIsNews = boardItemIsNews(list, bridgeFor(deviceId));
  for (const watcher of audienceFor(payload.subscription_id, deviceId)) {
    const mine = itemsFor(watcher, list);
    if (!mine.length || (watcher.boardScoped && !boardIsNews)) continue;
    deliverChanges(watcher, mine);
  }
  scheduleSync(deviceId);
  return true;
}

/** Who each kind of event wakes. A subscription flush is routed item by item,
 *  and that is the whole of what this client acts on. A kind with no entry
 *  here is an event this client does not act on. */
const EVENT_DISPATCHERS = new Map([["changes", dispatchItems]]);

/** Events about the machine rather than anything on it, acted on whether or
 *  not its change subscriptions are armed. */
const DEVICE_EVENTS = new Map([
  ["bridge.update_status", ({ type: _type, ...status }, deviceId) => {
    void rememberBridgeUpdateStatus(deviceId, status);
  }],
  ["models.changed", (_payload, deviceId) => modelsChangedOn(deviceId)],
]);

/** A change event off one device's session. Ignored entirely while that device
 *  is unarmed — an old bridge sends none, and a client that never greeted must
 *  behave as if it could not hear them. Returns whether the event was one we act
 *  on. */
export function dispatchChangeEvent(payload, deviceId = null) {
  const aboutTheDevice = DEVICE_EVENTS.get(payload?.type);
  if (aboutTheDevice && deviceId) {
    aboutTheDevice(payload, deviceId);
    return true;
  }
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

/** Greet in subscriptions mode, the only mode a bridge serves: there is no
 *  legacy greeting to start from and no second greeting to switch over. A
 *  bridge that does not advertise `changes.subscriptions` in its answer is
 *  asked for nothing. */
async function hello(call, strict = false) {
  try {
    return await call("session.hello", { client: clientDeclaration(), changes: "subscriptions" });
  } catch (error) {
    if (strict && !/unknown method|method not found/i.test(String(error?.message || ""))) throw error;
    return null; // an old bridge, or one that dropped mid-greeting
  }
}

/** Greet once. A slower old device can answer after its session has been
 *  replaced: its features and gap belong to that old session, not to this
 *  device now. */
async function negotiate(call, isCurrent, strict) {
  const greeting = await hello(call, strict);
  return { greeting, current: isCurrent() };
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
  setSubscriptionsMode(state, bridgeCapabilities(state.deviceId).changes.subscriptions);
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
    strict = false,
  } = {},
) {
  const { greeting, current } = await negotiate(call, isCurrent, strict);
  if (!current || !isCurrent()) return changeEventsArmed(deviceId);
  const adapter = install(selectAdapter(greeting));
  // Installing announces device state; a listener may issue another greeting
  // synchronously. That newer greeting now owns the shared bridge state too.
  if (!isCurrent()) return changeEventsArmed(deviceId);
  rememberGreetedReviews(deviceId, adapter, isCurrent);
  return publishGreeting(call, deviceId, greeting, adapter, onGreeting);
}

// Storage recovery must never hold a greeting open. Refresh again after the
// capability write, so an upgraded bridge fills reviews even if the first
// sync ran before this write. Superseded greetings cannot change the record.
function rememberGreetedReviews(deviceId, adapter, isCurrent) {
  void rememberReviewSupport(deviceId, adapter?.capabilities, isCurrent).then((written) => {
    if (written && adapter?.capabilities.reviews?.get && isCurrent()) refetchEverything(deviceId);
  });
}

/** Hear every greeting that settles what a machine's bridge can do — a
 *  reconnect's included — once capability readers see it. Answers the way to
 *  stop. For a surface that wanted something of a machine before it could be
 *  asked, and asks again when it greets. */
export function onBridgeGreeted(listener) {
  greetedListeners.add(listener);
  return () => greetedListeners.delete(listener);
}

const greetedListeners = new Set();

function announceGreeted(deviceId) {
  for (const listener of [...greetedListeners]) {
    try {
      listener(deviceId);
    } catch (error) {
      console.warn("a greeting listener threw:", error);
    }
  }
}

/** Publish the current selection to capability readers and subscriptions. */
function publishGreeting(call, deviceId, greeting, adapter, onGreeting) {
  const state = bridgeState(deviceId);
  state.apiVersion = greetingVersion(greeting);
  state.adapter = adapter || null;
  announceGreeted(state.deviceId);
  if (!state.adapter) return abandonBridge(state);
  // Into the cache, where the views that draw Needs you read it (#144).
  void rememberNeedsYouRule(deviceId, state.adapter.capabilities);
  // And whether Done there deletes the branch (#87).
  void rememberBranchDelete(deviceId, state.adapter.capabilities);
  // And whether its agents name the agent that made them (#221).
  void rememberConversationResetSupport(deviceId, state.adapter.capabilities);
  void rememberAgentLineageSupport(deviceId, state.adapter.capabilities);
  // And whether its sources can be edited in place (#228).
  void rememberSourceEditSupport(deviceId, state.adapter.capabilities);
  // And whether it measures its workspaces' sizes when asked (#273).
  void rememberWorkspaceSizeSupport(deviceId, state.adapter.capabilities);
  void rememberProjectFilesSupport(deviceId, state.adapter.capabilities);
  // And whether task body writes compare their original hash (#347).
  void rememberTaskChecklistSupport(deviceId, state.adapter.capabilities, () => bridgeFor(deviceId)?.adapter === adapter);
  onGreeting(greeting);
  armChangeEvents(greeting, deviceId);
  adoptGreetedSession(state, call);
  refetchEverything(deviceId);
  return changeEventsArmed(deviceId);
}
