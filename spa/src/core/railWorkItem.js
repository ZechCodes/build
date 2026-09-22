// Which work item a rail is standing on, and hearing it move.
//
// A rail is mounted on a ROUTE, and a route is not an entity: a branch is named
// by its project and its name, a workspace by the conversation it holds. The
// device's own rows answer that (core/cachedRows.js), and the row it names is
// the work item — its agents, what they are doing, what they have observed.
//
// This is the whole of the rail's reading: one row address, the watch on it,
// and the workspace list beside it for the one thing a row cannot say. Nothing
// here paints; it hands the rail a row and tells it when there is another.

import { ROW_RECORD_KIND, cachedRouteEntry } from "./cachedRows.js";
import { readCached, subscribeCache } from "./localCache.js";
import { issueAddress, readIssueRecord } from "./issueCache.js";

/** The other record read here: the machine's workspace list, which is the only
 *  thing that says what a workspace is mounted out of. */
const WORKSPACES_RECORD_KIND = "workspaces";

/**
 * One rail's reading of the cache.
 *
 * `standOn(row)` is handed each row as it is read. `reread()` is the rail's own
 * full re-read, asked for when a row this rail had none of arrives or when the
 * workspace list moves. `redrawConversation()` is called when — and only when —
 * the sources under the conversation moved, because the file chips in it are
 * addressed against them (core/threadLinks.js) and nothing else would re-draw
 * one. `alive()` is false once the rail is disposed: an answer landing after
 * that belongs to nobody.
 */
export function createRailWorkItem({
  context,
  railContext,
  cacheScope,
  callFor,
  named,
  standOn,
  reread,
  redrawConversation,
  alive,
}) {
  let mountedSources = [];
  let unwatchRow = null;
  let unwatchSources = null;
  let watchedRowId;
  let rereading = null;
  let rereadAgain = false;

  const rowAddress = (entityId) =>
    (entityId && (context.kind === "issue"
      ? cacheScope?.address(issueAddress(context.deviceId, entityId, "get"))
      : cacheScope?.address({ entityId, kind: ROW_RECORD_KIND }))) || null;

  /// The row itself, or nothing where this device holds none — a checkout
  /// nobody has claimed has no row anywhere, and the rail on it is the one that
  /// adopts on its first message.
  const cachedRow = async (entityId) => {
    const address = rowAddress(entityId);
    if (!address) return null;
    return (await readCached(address))?.value || null;
  };

  /// What the workspace list says about the workspace this rail is standing on:
  /// the name a message sent from here wears, and the sources its conversation's
  /// paths are written against. Neither is on the row — the row belongs to the
  /// conversation the workspace holds — so both come off the entry that
  /// resolved it.
  const learnWorkspaceFacts = (standing, entry) => {
    if (standing !== railContext || standing.kind !== "workspace") return;
    named(entry?.name);
    mountedSources = entry?.directories || [];
  };

  /// Which row in this device's cache is this rail's.
  ///
  /// A context handed its entity knows outright — a project's conversation is
  /// minted by the page, and an issue IS its own entity. Everything else is
  /// named by a route and resolved against the device's own rows: a branch by
  /// its project and name, a workspace by the conversation it holds, which is
  /// not the workspace's id (core/cachedRows.js).
  const entityIdFor = async (standing) => {
    if (standing.entityId) return standing.entityId;
    if (standing.kind === "issue") return standing.issueId || null;
    const entry = await cachedRouteEntry(context.deviceId, standing.feedRoute());
    learnWorkspaceFacts(standing, entry);
    return entry?.entityId || null;
  };

  /// Issues have no board row. Their detail pull is a writer to the issue's
  /// durable get record; its announcement makes this reader take it up. A
  /// warm record is returned immediately while that pull is still in flight.
  const read = async (entityId) => {
    if (!railContext.workItem) return cachedRow(entityId);
    void readIssueRecord({ deviceId: context.deviceId, issueId: entityId, sub: "get",
      read: () => railContext.workItem(callFor()), force: true }).catch(() => null);
    return cachedRow(entityId);
  };

  const takeUpRow = async (entityId) => {
    const row = await cachedRow(entityId);
    if (alive() && row) standOn(row);
  };

  /// One re-read at a time, and one more where something moved while it ran.
  ///
  /// Resolving a route walks every row the device holds, so a re-read per row
  /// written would cost N of those walks for the N rows one sync pass writes —
  /// on exactly the path the cache exists to make fast. Collapsing them loses
  /// nothing: the read that follows the burst sees every row in it.
  const rereadOnce = async () => {
    if (rereading) {
      rereadAgain = true;
      return rereading;
    }
    rereading = (async () => {
      do {
        rereadAgain = false;
        await reread();
      } while (rereadAgain && alive());
    })().finally(() => {
      rereading = null;
    });
    return rereading;
  };

  /// No row on this device answers to this route yet — a checkout nobody has
  /// claimed, or a boot whose first pass has not written the rows. Hear the
  /// rows as a whole until one of them is this rail's, and stop as soon as one
  /// is: this is the wide watch, and the narrow one is better.
  const watchForARowOfOurOwn = () => {
    const deviceAddress = cacheScope?.address({});
    if (!deviceAddress) return null;
    return subscribeCache(deviceAddress, (changed) => {
      if (changed.kind && changed.kind !== ROW_RECORD_KIND) return;
      void rereadOnce();
    });
  };

  /// Hear this row move. One watcher, re-pointed when the rail learns which row
  /// it is on — a branch adopted by its first message gains a row it did not
  /// have when it mounted.
  const watchRow = (entityId) => {
    if (watchedRowId === entityId) return;
    unwatchRow?.();
    unwatchRow = null;
    watchedRowId = entityId;
    const address = rowAddress(entityId);
    unwatchRow = address
      ? subscribeCache(address, () => void takeUpRow(entityId))
      : watchForARowOfOurOwn();
  };

  /// The workspace list moved. Read everything again, and where the sources
  /// moved with it say so: the conversation has to be re-drawn for chips that
  /// are addressed against them.
  const takeUpSources = async () => {
    const held = JSON.stringify(mountedSources);
    await reread();
    if (!alive() || JSON.stringify(mountedSources) === held) return;
    redrawConversation();
  };

  /// Hear the workspace list move. A source mounted or unmounted while the rail
  /// is open moves no row — the row is the conversation's — so the list is
  /// watched on its own, and only where there is a workspace to be mounted out
  /// of.
  const watchMountedSources = () => {
    if (context.kind !== "workspace" || unwatchSources) return;
    const address = cacheScope?.address({ entityId: "", kind: WORKSPACES_RECORD_KIND });
    if (!address) return;
    unwatchSources = subscribeCache(address, () => void takeUpSources());
  };

  return {
    entityIdFor,
    read,
    cachedRow,
    rowAddress,

    /** The entity the row watch is pointed at, for a reader that needs a name
     *  for this rail before any row has answered. */
    entityId: () => watchedRowId,

    /** The sources the workspace under this rail is mounted out of. */
    sources: () => mountedSources,

    /** Listen: to this rail's row, and to the list the sources are on. */
    watch(entityId) {
      watchRow(entityId);
      watchMountedSources();
    },

    unwatch() {
      unwatchRow?.();
      unwatchRow = null;
      unwatchSources?.();
      unwatchSources = null;
    },
  };
}
