// Which work item a rail is standing on, and hearing it move.
//
// A rail is mounted on a ROUTE, and a route is not an entity: a branch is named
// by its project and its name, a workspace by the conversation it holds. The
// device's own rows answer that (core/cachedRows.js), and the row it names is
// the work item — its agents, what they are doing, what they have observed.
// A conversation hidden from the inbox keeps its roster in the cached board's
// `runs` collection, which is also heard here.
//
// This is the whole of the rail's reading: one row address, the watch on it,
// and the workspace list beside it for the one thing a row cannot say. Nothing
// here paints; it hands the rail a row and tells it when there is another. A
// verb's answer about one agent is written here too, into the same record the
// rail reads, so the rail paints it the way it paints a push — and only where
// nothing has written that record since the verb went out.

import { ROW_RECORD_KIND, cachedRouteEntry } from "./cachedRows.js";
import { entityIdOf } from "./entityId.js";
import {
  cachedWriteOf,
  isCachedWrite,
  mergeCachedIfUnwritten,
  readCached,
  subscribeCache,
  updateCachedFeed,
} from "./localCache.js";
import { issueAddress, readIssueRecord } from "./issueCache.js";

/** The other record read here: the machine's workspace list, which is the only
 *  thing that says what a workspace is mounted out of. */
const WORKSPACES_RECORD_KIND = "workspaces";

/** Kinds whose row, where the device holds none of its own, is read off the
 *  board's `runs` instead (`cachedRow`). */
const READS_BOARD_RUNS = ["workspace", "project"];

/** A row with one agent rewritten, or null where the row does not name it or
 *  `rewrite` leaves it alone. The agents are on the row, or on its run. */
function rowWithAgent(row, agentId, rewrite) {
  const holder = Array.isArray(row?.agents) ? row : Array.isArray(row?.run?.agents) ? row.run : null;
  const index = holder ? holder.agents.findIndex((agent) => agent?.id === agentId) : -1;
  if (index < 0) return null;
  const rewritten = rewrite(holder.agents[index]);
  if (!rewritten) return null;
  const agents = holder.agents.map((agent, at) => (at === index ? rewritten : agent));
  return holder === row ? { ...row, agents } : { ...row, run: { ...row.run, agents } };
}

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
  let unwatchFeed = null;
  let unwatchSources = null;
  let watchedRowId;
  let rereading = null;
  let rereadAgain = false;

  const rowAddress = (entityId) =>
    (entityId && (context.kind === "issue"
      ? cacheScope?.address(issueAddress(context.deviceId, entityId, "get"))
      : cacheScope?.address({ entityId, kind: ROW_RECORD_KIND }))) || null;

  const feedAddress = () => cacheScope.address({ entityId: "", kind: "feed" });

  /// The row itself, or nothing where this device holds none — a checkout
  /// nobody has claimed has no row anywhere, and the rail on it is the one that
  /// adopts on its first message.
  const cachedRow = async (entityId) => {
    const address = rowAddress(entityId);
    if (!address) return null;
    const row = (await readCached(address))?.value;
    if (row || !READS_BOARD_RUNS.includes(context.kind)) return row || null;
    // An unwatched conversation still has an agent, but its
    // run is deliberately absent from the inbox's `items`. The same board
    // record keeps it in `runs`; read that cached roster for the workspace
    // without putting it back into the inbox as a visible row.
    const feed = (await readCached(feedAddress()))?.value;
    return (feed?.runs || []).find((run) => entityIdOf(run) === entityId) || null;
  };

  /// The record `cachedRow` reads this rail's row from, and the write it holds
  /// right now: the row's own, else the board's `runs`. Taken before a verb is
  /// sent, for `patchAgentIfUnwritten` to find unchanged after its answer.
  const rowWrite = async (entityId) => {
    const address = rowAddress(entityId);
    if (!address) return null;
    const record = await readCached(address);
    if (record?.value || !READS_BOARD_RUNS.includes(context.kind)) return { entityId, address, written: cachedWriteOf(record) };
    const feed = feedAddress();
    return { entityId, address: feed, written: cachedWriteOf(await readCached(feed)), runs: true };
  };

  /// Lay `rewrite(agent)` over one agent of this rail's row, in the record
  /// `rowWrite` named — only while that record still holds the write it saw.
  /// Whatever was written since — a push, another tab's answer — may be newer
  /// word than this answer, and nothing on a row says which, so the answer
  /// stands down and the record keeps what came. The record's watch hands the
  /// rail the row again.
  const patchAgentIfUnwritten = async (seen, agentId, rewrite) => {
    if (!seen) return;
    if (!seen.runs) {
      await mergeCachedIfUnwritten(seen.address, seen.written, (row) => rowWithAgent(row, agentId, rewrite));
      return;
    }
    await updateCachedFeed(seen.address, (feed, record) => {
      if (!isCachedWrite(record, seen.written)) return null;
      let moved = false;
      const runs = (feed?.runs || []).map((run) => {
        if (entityIdOf(run) !== seen.entityId) return run;
        const rewritten = rowWithAgent(run, agentId, rewrite);
        moved ||= Boolean(rewritten);
        return rewritten || run;
      });
      return moved ? { ...feed, runs } : null;
    });
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
    unwatchFeed?.();
    unwatchRow = null;
    unwatchFeed = null;
    watchedRowId = entityId;
    const address = rowAddress(entityId);
    unwatchRow = address
      ? subscribeCache(address, () => void takeUpRow(entityId))
      : watchForARowOfOurOwn();
    if (address && READS_BOARD_RUNS.includes(context.kind)) {
      unwatchFeed = subscribeCache(feedAddress(), () => void takeUpRow(entityId));
    }
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
    rowWrite,
    patchAgentIfUnwritten,

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
      unwatchFeed?.();
      unwatchFeed = null;
      unwatchSources?.();
      unwatchSources = null;
    },
  };
}
