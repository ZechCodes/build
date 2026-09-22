// The activity behind the folded runs of one conversation.
//
// A folded run is a head until the reader opens it, and what it opens onto is
// not always in hand: a page ships a bounded slice of a run and says in its
// digest how far the whole of it reaches. So a run the window does not hold is
// fetched over that span, written through to the local cache, and never asked
// for again — a run older than the newest message never changes, which is what
// makes it safe to keep until the entity is evicted.
//
// Two things live here, and they are the two a fold needs: which runs the
// reader has open, and what each open one holds. The tail run is not fetched at
// all — it is the one still being written, and the window is where its rows
// land.

import { createCachedBodies } from "./cachedBodies.js";

/** The local-cache kind one run's items are stored under. */
export const ACTIVITY_RECORD_KIND = "activity";

/** The most items one `thread.activity` answers with — the verb's own default,
 *  and what a run longer than that is paged back through. */
export const ACTIVITY_PAGE_LIMIT = 200;

/** Every conversation numbers its own items from one, so the run a record
 *  belongs to is named by the agent as well as by where the run starts. */
const runRecordSub = (agentId, fromSequence) => `${agentId || ""}:${fromSequence}`;

/**
 * The open runs of one conversation.
 *
 * Every one of these is spoken to in the key a fold names its run by — the
 * oldest sequence the window holds of it, which is where the run STARTS only
 * when the page did not cut one. The span a digest carries is what bridges the
 * two, and holding that here is what keeps a caller from translating: a
 * translation done wrong is an empty answer, and an empty answer looks exactly
 * like a run nobody has fetched.
 *
 * `itemsOf(runKey)` answers the items held for a run, or undefined — a paint
 * asks it and never waits. `open(digest)` is the only thing that does: it
 * fetches the run the digest describes unless it is already in hand. A stored
 * record landing calls `onChange`; that announcement, rather than the pull's
 * return value, is what tells the pane to paint its new rows.
 */
export function createActivityRuns({ deviceId, entityId, agentId, call, onChange = () => {} }) {
  const openRuns = new Set();
  // Where each fetched run starts and how far it reaches, learned from the
  // digest that opened it. The record is written under the start; the fold asks
  // with a key somewhere inside the span.
  const spans = new Map();
  // The runs being fetched right now. A reader pressing the same fold twice
  // while the first answer is still on its way is one ask, not two.
  const asking = new Set();

  const activityPage = (fromSequence, beforeSequence) =>
    call("thread.activity", {
      entity_id: entityId,
      ...(agentId ? { agent_id: agentId } : {}),
      from_sequence: fromSequence,
      through_sequence: spans.get(fromSequence),
      ...(beforeSequence === null ? {} : { before_sequence: beforeSequence }),
      limit: ACTIVITY_PAGE_LIMIT,
    });

  /// One run, whole: the newest page of its span, then the page above that, and
  /// so on until the daemon says there is nothing above. An answer holding
  /// nothing ends the walk whatever it says about what is above it — a page
  /// with no items names no seek to ask the next one with.
  async function fetchRun(fromSequence) {
    const items = [];
    let before = null;
    for (;;) {
      const page = await activityPage(fromSequence, before);
      const arrived = page.items || [];
      items.unshift(...arrived);
      if (!arrived.length || page.has_more !== true) return { fromSequence, items };
      before = page.oldest_sequence;
    }
  }

  const bodies = createCachedBodies({
    addressOf: (key) =>
      deviceId && entityId
        ? { deviceId, entityId, kind: ACTIVITY_RECORD_KIND, sub: runRecordSub(agentId, key) }
        : null,
    fetchMissing: (keys) => Promise.all(keys.map((key) => fetchRun(Number(key)))),
    valueOf: (run) => ({ key: run.fromSequence, value: { items: run.items } }),
    onChange,
  });

  /// Where the run a fold's key sits in starts, or nothing for a key no fetched
  /// run covers — a run the window holds whole, or one nobody has opened.
  const runStartAt = (runKey) => {
    const key = Number(runKey);
    for (const [fromSequence, throughSequence] of spans) {
      if (fromSequence <= key && key <= throughSequence) return fromSequence;
    }
    return null;
  };

  return {
    itemsOf(runKey) {
      const fromSequence = runStartAt(runKey);
      return fromSequence === null ? undefined : (bodies.read(fromSequence) || {}).items;
    },

    /** Hold what the run this digest describes contains, and answer whether
     *  that is news. A run already in hand is never asked for twice: it cannot
     *  have changed. An entity with no id has no conversation to ask about. */
    async open(digest) {
      const key = String(digest.from_sequence);
      if (!entityId || bodies.has(key) || asking.has(key)) return false;
      spans.set(digest.from_sequence, digest.through_sequence);
      asking.add(key);
      try {
        return (await bodies.ensure([key])).length > 0;
      } finally {
        asking.delete(key);
      }
    },

    isOpen: (key) => openRuns.has(String(key)),

    /** Flip a run's fold, and answer the side it landed on. */
    toggle(key) {
      const runKey = String(key);
      if (openRuns.delete(runKey)) return false;
      openRuns.add(runKey);
      return true;
    },

    openKeys: () => new Set(openRuns),
    dispose: () => bodies.dispose(),
  };
}
