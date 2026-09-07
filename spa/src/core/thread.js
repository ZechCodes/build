import { esc } from "./text.js";
import { renderMarkdown } from "./markdown.js";
import { RENDERED_FOLD_ATTRIBUTE, patchElement, patchInnerHtml } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { paintKeepingPlace, pinToBottom } from "./paintKeepingPlace.js";
import { EVENT_META, completionReportSections, eventLabel, isStartupEvent } from "./threadEvents.js";
import { activityRunSummary, digestCovering, firstLine, mergeActivityDigests } from "./activityDigest.js";
import {
  INTERRUPT_SEND_OPTION,
  autoGrow,
  composerHtml,
  composerPartIds,
  formatAttachmentSize,
  isImageAttachment,
  mountComposerAttachments,
  sendControlHtml,
} from "./composer.js";
import { mountSplitMenu } from "./splitButton.js";
import { outcomeMarkHtml } from "./outcomeMark.js";
import { providerLabel } from "./modelPicker.js";

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

function ordinal(day) {
  const lastTwoDigits = day % 100;
  if (lastTwoDigits >= 11 && lastTwoDigits <= 13) return `${day}th`;
  return `${day}${({ 1: "st", 2: "nd", 3: "rd" })[day % 10] || "th"}`;
}

function calendarDayNumber(date) {
  return Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / DAY_MS;
}

// eslint-disable-next-line complexity -- ratchet: formatRelativeDate is at 19, cap 10 — reduce it, then drop this line
export function formatRelativeDate(value, nowValue = new Date()) {
  const date = value instanceof Date ? value : new Date(value);
  const now = nowValue instanceof Date ? nowValue : new Date(nowValue);
  if (Number.isNaN(date.getTime()) || Number.isNaN(now.getTime())) return "";

  const elapsed = now.getTime() - date.getTime();
  if (elapsed >= 0 && elapsed < MINUTE_MS) return "Just now";
  if (elapsed >= 0 && elapsed < HOUR_MS) {
    const minutes = Math.floor(elapsed / MINUTE_MS);
    return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  }
  if (elapsed >= 0 && elapsed < DAY_MS) {
    const hours = Math.floor(elapsed / HOUR_MS);
    return `${hours} hour${hours === 1 ? "" : "s"} ago`;
  }

  const daysAgo = calendarDayNumber(now) - calendarDayNumber(date);
  if (daysAgo === 1) {
    const options = date.getMinutes()
      ? { hour: "numeric", minute: "2-digit" }
      : { hour: "numeric" };
    const time = date.toLocaleTimeString("en-US", options).replace(/\s/g, "").toLowerCase();
    return `Yesterday at ${time}`;
  }
  if (daysAgo > 1 && daysAgo < 7) {
    return date.toLocaleDateString("en-US", { weekday: "long" });
  }

  const month = date.toLocaleDateString("en-US", { month: "long" });
  const label = `${month} ${ordinal(date.getDate())}`;
  return date.getFullYear() === now.getFullYear() ? label : `${label}, ${date.getFullYear()}`;
}

function timeHtml(createdAt) {
  if (!createdAt) return "";
  const label = formatRelativeDate(createdAt);
  if (!label) return "";
  return `<time datetime="${esc(createdAt)}">${esc(label)}</time>`;
}

// Client half of the thread cursor: the detail polls (plan.get / run.get every
// 1.6s) would otherwise re-ship the whole forever-growing conversation over
// E2EE on every tick. The cache holds a WINDOW over one entity's conversation —
// the newest items the daemon paged out, widened upwards as the reader scrolls
// back — tells the caller which cursor to send, and folds each delta back into
// a full thread for rendering. A window that no longer reaches the newest item,
// or that holds more than the conversation does (bridge restart, entity swap,
// dropped delta), drops it for a refetch of a window the same height.
// How much conversation a first load asks for. The daemon clamps whatever it
// hears, so this is a request rather than a promise — but it has to be made:
// a poll that names no bound gets the conversation whole, which is the only
// answer a client written before paging could reconcile.
export const FIRST_PAGE_ITEMS = 20;

export const THREAD_RECORD_KIND = "thread";

// What a MUTATION asks its answer to carry. Every mutation RPC answers with
// the whole entity, conversation included, and no caller here reads that
// answer — the refresh that follows is what paints. The page is asked for
// anyway: an answer nobody reads must still not grow with the conversation,
// and a call that names no bound gets every item it ever held. A cursor is no
// use here, since the answer has to stand on its own for whoever starts
// reading it.
export const MUTATION_THREAD_PAGE = Object.freeze({ thread_limit: FIRST_PAGE_ITEMS });

// What a surface asks for when it renders no conversation at all but still
// polls a detail RPC — the branch surface, whose conversation is the rail's
// beside it, and the console, which wants only a directory to stand in. Naming
// no bound would ship every item the conversation ever held on every tick; the
// smallest page the daemon will cut still carries the bounded fields off the
// thread these surfaces do read (the diff revision a comment anchors to).
export const SMALLEST_THREAD_PAGE = Object.freeze({ thread_limit: 1 });

export const threadItemKey = (item) => String(item?.data?.sequence ?? "");

export function createThreadCache() {
  let accumulatedItems = [];
  // What each activity run that touches the window totals, keyed by the run's
  // first sequence. A page ships a bounded slice of a run and says here what
  // the whole of it came to; a forward delta says nothing about a run, which is
  // why these are held rather than recomputed from what is in hand.
  let activityDigests = [];
  // Whether the daemon said there is conversation above the window. Only a
  // paged answer knows; a forward delta says nothing about the far end.
  let olderItemsRemain = false;
  // The newest counter value the daemon has named that this cache has already
  // taken delivery of. Usually the top of the window — but an item mutated in
  // place BELOW the window is taken delivery of by being left out of it (see
  // `theWindowMayTake`), and the cursor still has to move past its bump or the
  // daemon re-ships it on every poll for as long as the view is open.
  let deliveredSequence = 0;
  // How long the daemon last said the whole conversation is, or null while no
  // window is open. Held because a REMOVAL is the one change the wire has no
  // other word for (see `growsByWhatItWasTold`).
  let knownTotalItems = null;
  // How tall the window was when it last broke, or 0 with nothing to recover.
  // See `forgetTheBrokenWindowButNotItsHeight`.
  let itemsToRecover = 0;

  // The bridge bumps `updated_sequence` (drawn from the same counter as
  // `sequence`) when it mutates a message in place — marking it seen,
  // resolving it with a revision — so the cursor must cover the newest
  // counter value any held item has touched, not just the newest creation.
  const itemCursorSequence = (item) =>
    Math.max(item.data?.sequence || 0, item.data?.updated_sequence || 0);

  const highestCursorSequence = (items) =>
    items.reduce((highest, item) => Math.max(highest, itemCursorSequence(item)), 0);

  /// The part of an arrival the window is allowed to fold in.
  ///
  /// The daemon's forward cursor selects on the newest counter value an item
  /// has TOUCHED, so mutating an old message in place — marking it seen,
  /// resolving it with a revision, answering its offer — re-ships that message
  /// however far below the window it was written. Folding it back in by
  /// creation sequence would seat it under the window's floor with everything
  /// between them missing: a hole the two-ended check below cannot see, because
  /// the mutated item's own bump IS the newest sequence the daemon names. Worse,
  /// it would move the floor down to the far side of the hole, so one scroll
  /// back would answer with the handful of items above the mutated one, say
  /// there is no more, and bury the rest of the conversation for the life of
  /// the view.
  ///
  /// So an arrival from under the window stays out of it. The reader meets its
  /// current state the moment they scroll back far enough to fetch it.
  const theWindowMayTake = (arrivedItems) => {
    if (!accumulatedItems.length) return arrivedItems;
    const floor = accumulatedItems[0].data?.sequence || 0;
    return arrivedItems.filter((item) => (item.data?.sequence || 0) >= floor);
  };

  /// Whether a payload's `has_more` is still an answer about the window in
  /// hand. It answers one question — is there anything above the payload's own
  /// first item — so it holds only while that item is still the window's floor.
  /// With no window open, the payload is the one about to become it.
  ///
  /// The distinction is not academic: a repaint folds the payload in hand back
  /// through the cache, and the payload in hand stays the page the window was
  /// opened on until the next poll replaces it with a delta. Once the reader
  /// has scrolled back, that page speaks for a floor the window has already
  /// lifted past, and taking its answer would put them at a top they have
  /// already reached — every further scroll gesture asking for a page the
  /// daemon has already said is not there.
  const speaksForTheWindowsFloor = (arrivedItems) => {
    if (!accumulatedItems.length) return true;
    return (arrivedItems[0]?.data?.sequence || 0) === (accumulatedItems[0].data?.sequence || 0);
  };

  /// Drop the window: what is held, what was said about either end of it, and
  /// how long the conversation was. The next poll opens a fresh one on the
  /// newest items, which is where a conversation is opened.
  const forgetTheWindow = () => {
    accumulatedItems = [];
    activityDigests = [];
    olderItemsRemain = false;
    deliveredSequence = 0;
    knownTotalItems = null;
    itemsToRecover = 0;
  };

  /// Drop a window that turned out to be broken, remembering how tall it was.
  ///
  /// A reader who has scrolled back is reading history, and the window they are
  /// reading it in is the only record of how far back they went — the daemon's
  /// detail poll takes a size, not a floor, so the height is what can be asked
  /// for again. Reopening on a first page instead would take that history off
  /// the screen with nothing said: the surfaces write the reader's scroll
  /// offset back after a repaint, and a timeline a quarter as tall clamps it to
  /// a point in the conversation they were never at, with the passage they were
  /// reading only reachable by scrolling back page by page a second time.
  ///
  /// So a break asks for the window again rather than for a first page, and the
  /// recovery is invisible. This is the one thing kept across a break: what was
  /// held is suspect, and what it was a window on may not even be the same
  /// conversation any more, but how much the reader had open is a fact about
  /// the reader.
  const forgetTheBrokenWindowButNotItsHeight = () => {
    const heldItemCount = accumulatedItems.length;
    forgetTheWindow();
    itemsToRecover = heldItemCount;
  };

  const mergeArrivals = (arrivedItems) => {
    // Keyed by creation sequence so a replay never grows the list, while an
    // arrived copy replaces the held one — the bridge re-ships an item
    // exactly when it holds newer state (seen, resolved) for it.
    const mergedBySequence = new Map(accumulatedItems.map((item) => [item.data?.sequence, item]));
    for (const arrived of arrivedItems) {
      mergedBySequence.set(arrived.data?.sequence, arrived);
    }
    return [...mergedBySequence.values()].sort(
      (a, b) => (a.data?.sequence || 0) - (b.data?.sequence || 0),
    );
  };

  /// Whether a window is still one unbroken run of the conversation's newest
  /// items — the only shape the forward cursor is safe on top of.
  ///
  /// Counting is not the test any more: a paged client holds fewer items than
  /// `thread_total` on purpose, and treating that as a loss would refetch the
  /// whole conversation every 1.6s, which is the exact cost paging exists to
  /// avoid. Contiguity WITHIN the window is kept by construction — a forward
  /// delta carries everything after the cursor, an older page carries the items
  /// immediately before the front — so what is left to check is the two ends:
  /// the cache must have taken delivery of everything up to the newest sequence
  /// the daemon names (falling short of it means a delta went missing), and the
  /// window can never be larger than the conversation it is a window on (a
  /// smaller whole means the conversation restarted, was trimmed, or belongs to
  /// somebody else now).
  ///
  /// Delivery rather than the top of the window, because the two part company:
  /// an item mutated below the floor is delivered and deliberately not held.
  ///
  /// A PAGE is held to the size end only. It is the answer that OPENS a window
  /// rather than one that extends it, so there is no delta it could have lost
  /// — the cursor is read straight back off the items it shipped. Meanwhile
  /// `thread_last_sequence` names the newest counter value in the whole
  /// conversation, which an in-place bump routinely puts on an item the page
  /// deliberately left out: an old message marked seen, an old comment
  /// resolved, with nothing posted since. That is the state an idle
  /// conversation is normally opened in, so holding a page to the delivery end
  /// would fail the check on every first load — resetting the window every
  /// tick, leaving the reader the newest page with no scroll-back, and never
  /// letting the cursor engage. The bump arrives from under the floor on the
  /// next poll, as a delta, and moves delivery past itself there.
  const holdsAnUnbrokenRunEndingAtTheNewest = (merged, delivered, threadPayload, arrivedAsAPage) => {
    const newest = threadPayload.thread_last_sequence;
    // A daemon old enough not to name its newest sequence leaves only the size
    // check to go on.
    if (!arrivedAsAPage && newest != null && delivered !== newest) return false;
    return merged.length <= threadPayload.thread_total;
  };

  /// How many of an arrival's items are conversation that did not exist when
  /// the window was last checked. The forward cursor re-ships an item it merely
  /// mutated in place, and that item was counted the first time it arrived —
  /// only a creation makes the conversation longer, and a creation is exactly
  /// an item whose own sequence is past everything delivered so far.
  const createdSince = (arrivedItems, delivered) =>
    arrivedItems.filter((item) => (item.data?.sequence || 0) > delivered).length;

  /// Whether the conversation is as long as what the cache has been told about
  /// it — the only signal the wire carries for an item that was DELETED.
  ///
  /// Nothing arrives to unsay a removed item. `Thread::remove_doc_comment`
  /// takes it out of the conversation and spends no sequence doing so, so the
  /// next delta is empty, the newest sequence is where it was, and the one
  /// thing that moves is `thread_total` going down by one. A window is shorter
  /// than the whole by design, so the size check cannot hear that: a reviewer
  /// deleting their own comment would leave it drawn on the rail for the life
  /// of the view, because no later poll ever mentions it again.
  ///
  /// So the length is predicted instead of compared: whatever the daemon said
  /// last, plus the items it has since shipped that are new. Falling short of
  /// that means the conversation lost something — a deletion inside the window
  /// or below it — and the window is dropped for a refetch. Predicting also
  /// catches the tick that deletes one item and posts another, which leaves the
  /// count alone and would otherwise pass unnoticed.
  ///
  /// Only falling SHORT is a loss. A conversation longer than predicted is a
  /// delta the daemon bounded, which the delivery end of the check answers.
  const growsByWhatItWasTold = (threadPayload, arrivedItems, delivered) => {
    if (knownTotalItems == null) return true;
    return threadPayload.thread_total >= knownTotalItems + createdSince(arrivedItems, delivered);
  };

  /// The thread a caller renders: the payload, the items the window holds, and
  /// what the daemon said each activity run over them totals. One shape for
  /// every answer, so items and digests can never be handed out out of step.
  const threadOver = (threadPayload, items, digests) => ({ ...threadPayload, items, activityDigests: digests });

  return {
    // Extra params for the next plan.get / run.get: the last sequence held, or
    // — with no window open (first load, or after a reset) — how much of the
    // newest conversation to open one on. A window dropped for a break asks for
    // its own height back, so the reader keeps the history they had scrolled to
    // (`forgetTheBrokenWindowButNotItsHeight`); the daemon clamps that like any
    // other request.
    cursorParam() {
      return accumulatedItems.length
        ? { thread_after_sequence: deliveredSequence }
        : { thread_limit: Math.max(FIRST_PAGE_ITEMS, itemsToRecover) };
    },
    // Extra params for the next thread.page: the seek for the page above the
    // window, or nothing while there is no window to widen.
    olderPageParam() {
      if (!accumulatedItems.length) return null;
      return { before_sequence: accumulatedItems[0].data?.sequence };
    },
    // Whether asking for that page is worth it — the daemon's own answer.
    hasOlderItems() {
      return olderItemsRemain;
    },
    // The oldest sequence the window holds, or null while it holds no window
    // at all. What a read report is measured against: reaching the end of a
    // window says the reader was shown what is in it, and nothing about the
    // conversation below. Null is the honest answer for a conversation that
    // arrived whole, where the end of what is held IS the end.
    windowFloorSequence() {
      if (!accumulatedItems.length) return null;
      return accumulatedItems[0].data?.sequence ?? null;
    },
    // Fold a polled thread payload into the cache and return a thread whose
    // `items` is the complete accumulated list. Never mutates the payload.
    absorb(threadPayload) {
      if (!threadPayload) {
        forgetTheWindow();
        return threadPayload;
      }
      const arrivedItems = threadPayload.items || [];
      const digests = mergeActivityDigests(activityDigests, threadPayload);
      activityDigests = digests;
      // `has_more` is what a page carries and a forward delta does not, so it
      // is also what tells the two kinds of answer apart.
      const arrivedAsAPage = threadPayload.has_more != null;
      // A paged answer is the only one that knows what lies above it; a bare
      // forward delta leaves the standing answer alone, and so does a page that
      // no longer speaks for the window's floor.
      if (arrivedAsAPage && speaksForTheWindowsFloor(arrivedItems)) {
        olderItemsRemain = threadPayload.has_more === true;
      }
      if (threadPayload.thread_total == null) {
        // An uncursored (full) response is authoritative: replace, don't merge.
        // It names no length, so there is none to hold the next one to.
        accumulatedItems = [...arrivedItems];
        deliveredSequence = highestCursorSequence(accumulatedItems);
        knownTotalItems = null;
        return threadOver(threadPayload, accumulatedItems, digests);
      }
      // A window is opened by a PAGE and only by a page. A forward delta
      // carries what is newer than the cursor it was asked with, which says
      // nothing about how far back the conversation goes — only a paged answer
      // knows that, and says so with `has_more`. So a delta arriving on an
      // empty cache is rendered and forgotten rather than taken as the window:
      // the cache holds no window here because the last absorb reset it (or
      // the reader just switched agents), and the delta is the tail of a
      // conversation whose floor it cannot name. Seating it as the window would
      // pass both ends of the check trivially — it reaches the newest item, and
      // a handful of items is never more than the whole — leaving the reader
      // with those few messages, a cursor past them, and no page above, which
      // no later delta ever brings the rest back to. The repaints that make
      // this reachable are ordinary: pressing a bubble, or leaving the chat and
      // coming back, folds the payload in hand through the cache again.
      if (!accumulatedItems.length && !arrivedAsAPage) {
        return threadOver(threadPayload, arrivedItems, digests);
      }
      const merged = mergeArrivals(theWindowMayTake(arrivedItems));
      // Everything the delta carried is delivered, whether the window took it
      // or left it below the floor.
      const delivered = Math.max(
        deliveredSequence,
        highestCursorSequence(merged),
        highestCursorSequence(arrivedItems),
      );
      const sound =
        holdsAnUnbrokenRunEndingAtTheNewest(merged, delivered, threadPayload, arrivedAsAPage) &&
        growsByWhatItWasTold(threadPayload, arrivedItems, deliveredSequence);
      if (!sound) {
        // A real gap: render what we have this tick, but drop the cache so the
        // next poll refetches a window this tall from the newest item down and
        // self-heals under a reader who never sees it happen.
        forgetTheBrokenWindowButNotItsHeight();
        return threadOver(threadPayload, merged, digests);
      }
      accumulatedItems = merged;
      deliveredSequence = delivered;
      knownTotalItems = threadPayload.thread_total;
      return threadOver(threadPayload, accumulatedItems, digests);
    },
    // Widen the window upwards with a `thread.page` answer and return the whole
    // of it, or nothing when the page no longer belongs above the window.
    // Never mutates the payload, and never moves the forward cursor: that one
    // reads what has been delivered, and history arriving late is not news.
    //
    // `seek` is the `olderPageParam()` the page was asked for with, and it is
    // what makes the answer safe to fold in: a page carries the items
    // immediately before the seek it was fetched at, so it abuts this window
    // only while the window's floor is still that seek. A round trip is long
    // enough for it to stop being — the reader switched agents, or a poll
    // tripped the gap check and the poll after it opened a fresh window on the
    // newest items. Folding the page in then would seat it under a floor it was
    // never below, with everything between them missing and the floor left on
    // the far side of the hole, so scrolling back would walk downward and the
    // skipped items could never be asked for again. A hole, dressed as history
    // — and one neither end of the window is short enough to give away. So a
    // page that has outlived its seek is dropped; the reader's next scroll asks
    // for the page this window actually wants.
    absorbOlderPage(pagePayload, seek) {
      if (!pagePayload || !accumulatedItems.length) return null;
      if (!seek || seek.before_sequence !== accumulatedItems[0].data?.sequence) return null;
      if (pagePayload.has_more != null) olderItemsRemain = pagePayload.has_more === true;
      accumulatedItems = mergeArrivals(pagePayload.items || []);
      activityDigests = mergeActivityDigests(activityDigests, pagePayload);
      return threadOver(pagePayload, accumulatedItems, activityDigests);
    },
    reset() {
      forgetTheWindow();
    },
    // The window as a value the local cache can hold across sessions, or null
    // while none is open. What seedWindow takes back.
    readWindow() {
      if (!accumulatedItems.length) return null;
      return { items: accumulatedItems, olderItemsRemain, deliveredSequence, knownTotalItems, activityDigests };
    },
    // Open a saved window in an empty cache. Only an empty one: a conversation
    // already live outranks anything the disk remembers. After a seed the
    // cursor is a forward delta, and the standing soundness checks self-heal
    // whatever the time away made stale — a break refetches, invisibly.
    seedWindow(saved) {
      if (accumulatedItems.length || deliveredSequence) return false;
      if (!saved || !Array.isArray(saved.items) || !saved.items.length) return false;
      accumulatedItems = [...saved.items];
      olderItemsRemain = !!saved.olderItemsRemain;
      deliveredSequence = saved.deliveredSequence || highestCursorSequence(accumulatedItems);
      knownTotalItems = saved.knownTotalItems ?? null;
      activityDigests = saved.activityDigests || [];
      return true;
    },
  };
}

/** A bare thread payload (a first page fetched out of band, with no cache in
 *  hand) shaped as the saved window seedWindow takes — the background syncer's
 *  way of refreshing a persisted conversation without owning one. Null for a
 *  payload holding nothing: an empty window is not worth a seed. */
export function windowFromThreadPayload(threadPayload) {
  const items = (threadPayload && threadPayload.items) || [];
  if (!items.length) return null;
  const deliveredSequence = items.reduce(
    (highest, item) => Math.max(highest, item.data?.sequence || 0, item.data?.updated_sequence || 0),
    0,
  );
  return {
    items,
    olderItemsRemain: threadPayload.has_more === true,
    deliveredSequence,
    knownTotalItems: threadPayload.thread_total ?? null,
    activityDigests: mergeActivityDigests([], threadPayload),
  };
}

export function currentRevisionId(thread, artifact) {
  const revisions = (thread && thread.revisions) || [];
  return [...revisions].reverse().find((revision) => revision.artifact === artifact)?.id || null;
}

function anchorLabel(anchor) {
  if (!anchor) return "";
  const path = anchor.path || (anchor.artifact === "plan" ? "plan" : "diff");
  const start = anchor.line_start;
  const end = anchor.line_end;
  const lines = start == null ? "" : start === end ? `:${start}` : `:${start}-${end}`;
  const heading = anchor.heading_path && anchor.heading_path.length ? ` · ${anchor.heading_path.join(" › ")}` : "";
  return `<div class="thread-anchor">${esc(path + lines + heading)}${anchor.snippet ? ` · “${esc(anchor.snippet.replace(/\s+/g, " ").slice(0, 120))}”` : ""}</div>`;
}

// What the bridge called a harness before it had wire tokens for them. Kept so
// a conversation recorded then still reads as the harness it ran on.
const LEGACY_PROVIDER_IDS = { "Codex CLI": "codex", Claude: "claude" };

function harnessLabel(thread, override) {
  const raw = override || (thread && thread.sessions && thread.sessions.at(-1)?.provider) || "";
  return providerLabel(LEGACY_PROVIDER_IDS[raw] || raw);
}

// eslint-disable-next-line complexity -- ratchet: linkLocation is at 12, cap 10 — reduce it, then drop this line
function linkLocation(link) {
  if (link.kind !== "file") {
    return link.path || link.implementation_id || link.run_id || link.worktree_id || link.sha || link.recovery_id || "Open";
  }
  const start = link.line_start;
  const end = link.line_end;
  const lines = start == null ? "" : start === end || end == null ? `:${start}` : `:${start}-${end}`;
  return `${link.path || "file"}${lines}`;
}

function linksHtml(links) {
  if (!links || !links.length) return "";
  return `<div class="thread-references">${links
    // eslint-disable-next-line complexity -- ratchet: this callback is at 13, cap 10 — reduce it, then drop this line
    .map((link) => {
      const attributes = [
        `data-kind="${esc(link.kind || "")}"`,
        link.path ? `data-path="${esc(link.path)}"` : "",
        link.issue_id ? `data-issue-id="${esc(link.issue_id)}"` : "",
        link.plan_id ? `data-plan-id="${esc(link.plan_id)}"` : "",
        link.stage_id ? `data-stage-id="${esc(link.stage_id)}"` : "",
        link.implementation_id ? `data-implementation-id="${esc(link.implementation_id)}"` : "",
        link.run_id ? `data-run-id="${esc(link.run_id)}"` : "",
        link.worktree_id ? `data-worktree-id="${esc(link.worktree_id)}"` : "",
        link.sha ? `data-sha="${esc(link.sha)}"` : "",
        link.recovery_id ? `data-recovery-id="${esc(link.recovery_id)}"` : "",
        link.line_start != null ? `data-line-start="${Number(link.line_start)}"` : "",
        link.line_end != null ? `data-line-end="${Number(link.line_end)}"` : "",
      ].filter(Boolean).join(" ");
      return `<button type="button" class="thread-reference" ${attributes}>${esc(linkLocation(link))}</button>`;
    })
    .join("")}</div>`;
}

/// Attachment bytes already fetched, keyed by path — and `null` for a path the
/// bridge would not give up.
///
/// Safe to hold forever within a session: an attachment is content-addressed
/// and immutable, so a path always means the same bytes. Worth holding, because
/// the surfaces re-render the whole timeline on every poll and a conversation
/// full of screenshots would otherwise re-fetch all of them every second and a
/// half. Bounded so a long session cannot grow without limit.
///
/// The renderer reads the failures, so a picture that will not come says the
/// same thing on every render. The bytes stay out of the markup — a screenshot
/// is megabytes, and re-serialising it into the timeline string 37 times a
/// minute would cost more than it saves. The picture on the page keeps them:
/// `patchElement` leaves a loaded `<img>` its `src`.
const attachmentDataUrls = new Map();
const ATTACHMENT_CACHE_MAX = 40;

function rememberAttachment(path, dataUrl) {
  if (attachmentDataUrls.size >= ATTACHMENT_CACHE_MAX) {
    attachmentDataUrls.delete(attachmentDataUrls.keys().next().value);
  }
  attachmentDataUrls.set(path, dataUrl);
}

/// The files a message came with.
///
/// An image is shown, not linked: the reason to attach a screenshot is that
/// looking at it IS the message, and a chip reading "screenshot.png" makes the
/// reader click to find out what they were told. Everything else is a chip that
/// downloads, since the browser has nothing useful to do with a tarball.
///
/// `src` is left empty here and filled by [`wireThreadAttachments`] — the
/// timeline is a string, and the bytes are a round trip away. A picture already
/// asked for and refused is drawn as refused, so that a repaint of the same
/// conversation is the same markup down to the class.
function attachmentsHtml(attachments) {
  if (!attachments || !attachments.length) return "";
  return `<div class="thread-attachments">${attachments
    .map((attachment) => {
      const path = esc(attachment.path || "");
      const name = esc(attachment.name || attachment.path || "file");
      if (isImageAttachment(attachment.mime)) {
        const refused = attachmentDataUrls.get(attachment.path) === null ? " unavailable" : "";
        return `<figure class="thread-attachment-figure${refused}">
          <img class="thread-attachment-image" data-attachment-path="${path}" alt="${name}">
          <figcaption>${name}</figcaption>
        </figure>`;
      }
      return `<button type="button" class="thread-attachment" data-attachment-path="${path}" data-attachment-name="${name}">
        <span class="thread-attachment-name">${name}</span>
        <span class="thread-attachment-size">${esc(formatAttachmentSize(attachment.size))}</span>
      </button>`;
    })
    .join("")}</div>`;
}

/// What the reader has picked on an offer and not yet sent, keyed by the offer
/// it was picked on.
///
/// Held outside the markup because the conversation repaints every second and a
/// half: a selection lives in the render, so it has to survive one — the same
/// reason the composer's draft is not kept in the DOM either. Dropped once the
/// choice is on the record, which is the moment the chips stop being pressable.
const pendingChoices = new Map();

/// The offers being submitted right now. They read as shut while the daemon
/// decides, so a second press cannot send the same choice twice.
const sendingChoices = new Set();

/// What a pick is filed under. Every conversation numbers its messages from
/// one, so the message id alone would put one thread's picks on another's
/// chips the moment the reader switched agents.
const offerKey = (threadId, messageId) => `${threadId || ""}::${messageId || ""}`;

/// What is marked on an offer's chips: what was actually sent, once there is
/// such a thing; what is picked and not yet sent, while it can still be sent;
/// and nothing at all on an offer that went by unanswered — picking is not
/// choosing, so an offer overtaken mid-pick leaves no mark.
const chosenOn = (message, live, key) => {
  if ((message.selected_options || []).length) return new Set(message.selected_options);
  return live ? pendingChoices.get(key) || new Set() : new Set();
};

/// The actions the agent suggested taking in answer to its message.
///
/// `live` is the whole of whether they can be pressed: an offer is answerable
/// only while it is the last thing said and nothing has been chosen on it, so
/// anything said afterwards — by either side — leaves it dim exactly as it
/// stands. A choice already made keeps its chips marked, because that mark is
/// the conversation's only record of what the reader pressed.
function optionsHtml(message, live, key) {
  const options = message.options || [];
  if (!options.length) return "";
  const answered = (message.selected_options || []).length > 0;
  const shut = answered || !live;
  const chosen = chosenOn(message, live, key);
  const chips = options
    .map(
      (option) => `<button type="button" class="thread-option${chosen.has(option.id) ? " chosen" : ""}"
        data-option-id="${esc(option.id)}" aria-pressed="${chosen.has(option.id)}"${shut ? " disabled" : ""}>
        <span class="thread-option-label">${esc(option.label || "")}</span>
      </button>`,
    )
    .join("");
  // The send is dropped once the choice is recorded rather than dimmed with the
  // chips: a shut offer with nothing chosen can still be read as an offer that
  // went by, but an answered one has nothing left to send.
  const send = answered
    ? ""
    : `<button type="button" class="thread-options-send"${shut || !chosen.size ? " disabled" : ""}>Send</button>`;
  return `<div class="thread-options" data-message="${esc(message.id || "")}" data-offer="${esc(key)}">
    <div class="thread-option-list">${chips}</div>
    ${send}
  </div>`;
}

/// What an agent reported through `done`, in the vocabulary of the event each
/// outcome replaced — so a completion reads as "reported done" with the same
/// tick and the same tone it always did, and a blocker as the blocker it always
/// was. Keyed by the wire token (`bridge/src/thread.rs`, `MessageOutcome`).
const OUTCOME_META = {
  completed: EVENT_META.done,
  blocked: EVENT_META.blocked,
  failed: EVENT_META.run_failed,
};

/// The outcome a message reports, as a marker on the message that reports it.
///
/// An outcome is a status the agent attached to its own words, so the marker
/// rides the card rather than standing beside it as a second record. A token
/// this client has no meta for still marks the message — the reader learns an
/// outcome was reported, in the agent's own token, rather than reading the
/// message as an ordinary reply.
function outcomeMarkerHtml(outcome, agentLabel) {
  if (!outcome) return "";
  const meta = OUTCOME_META[outcome] || { label: String(outcome).replaceAll("_", " "), icon: "•" };
  const label = meta.label.replace(/^Agent\b/, agentLabel);
  return `<div class="thread-outcome ${meta.tone || ""}" data-outcome="${esc(outcome)}">
    <span class="thread-outcome-icon" aria-hidden="true">${esc(meta.icon)}</span>
    <strong>${esc(label)}</strong>
  </div>`;
}

// eslint-disable-next-line complexity -- ratchet: messageHtml is at 11, cap 10 — reduce it, then drop this line
function messageHtml(message, agentLabel = "Agent", liveOptions = false, offer = "") {
  const user = message.role === "user";
  const status = user
    ? `<span class="thread-status">${message.seen_at ? "Seen" : "Unread"}${message.resolved_by_revision ? ` · <button class="thread-revision-link" data-revision="${esc(message.resolved_by_revision)}">Resolved in ${esc(message.resolved_by_revision)}</button>` : ""}</span>`
    : "";
  // `done` is message metadata, not a presentation type: on a thread written
  // before outcomes were message statuses it flags the send that followed the
  // timeline's done event, and such a message renders like every other one.
  // What marks a message is `outcome` — the whole record of a reported outcome,
  // carrying the structured handoff the done event used to.
  // renderMarkdown escapes all input before adding its fixed safe tag set.
  return `<article class="thread-message thread-comment ${user ? "user" : "agent"}">
    <span class="thread-avatar" aria-hidden="true">${user ? "Y" : "A"}</span>
    <div class="thread-comment-card">
      <div class="thread-message-head"><span><strong>${user ? "You" : esc(agentLabel)}</strong> commented ${timeHtml(message.created_at)}</span>${status}</div>
      ${outcomeMarkerHtml(message.outcome, agentLabel)}
      ${anchorLabel(message.anchor)}
      ${message.body ? `<div class="thread-body markdown">${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ renderMarkdown(message.body)}</div>` : ""}
      ${completionReportHtml(message.completion_report)}
      ${attachmentsHtml(message.attachments)}
      ${linksHtml(message.links)}
      ${optionsHtml(message, liveOptions, offer)}
    </div>
  </article>`;
}

/// The agent's handoff, as a card.
///
/// `done` is asked for a completion report, and the report renders wherever the
/// record of that completion is: on the outcome message that reports it, and on
/// the `Done` event of a thread written before outcomes were message statuses.
/// Either way it is the same card — the critical files, the decisions a
/// reviewer would otherwise reverse-engineer, the risks, and what was
/// deliberately left alone. Every line is the agent's words — escaped.
function completionReportHtml(report) {
  const sections = completionReportSections(report);
  if (!sections.length) return "";
  return `<div class="completion-report">${sections
    .map(
      (section) =>
        `<div class="completion-section"><div class="completion-title">${esc(section.title)}</div>
        <ul>${section.items.map((item) => `<li>${esc(item)}</li>`).join("")}</ul></div>`,
    )
    .join("")}</div>`;
}

/// What a tool call's answer reported, as a mark on the call's own row.
///
/// The call and the answer are one row, so the row has three states to say and
/// says them here rather than as a second row underneath. Only the error mark
/// takes a colour, and only the MARK does: the row stays toneless, because
/// activity asks the reader for nothing and a tool call that failed still
/// doesn't — the agent was told, and the agent calling the human is what a
/// blocker is for.
const TOOL_OUTCOME_MARKS = {
  ok: { mark: "ok", label: "The tool answered" },
  error: { mark: "error", label: "The tool reported an error" },
  unanswered: { mark: "unanswered", label: "No answer arrived" },
};

/// The mark, or nothing at all.
///
/// Nothing is the PENDING state — the call is still running — and it is what an
/// unanswered-so-far row carries, what every row written before calls and
/// answers were one row carries, and what a state this build has no name for
/// carries: the additive wire read in the client's direction, where the safe
/// reading of a token from a newer daemon is the one that claims nothing.
function toolOutcomeHtml(outcome) {
  const entry = TOOL_OUTCOME_MARKS[outcome];
  if (!entry) return "";
  return outcomeMarkHtml(entry.mark, entry.label);
}

/// Activity, folded.
///
/// Reasoning, tool calls, tool results, narration and background tasks are the
/// agent working, not the agent addressing anyone — the daemon classes them all
/// as status, so they move no unread count and pull nobody in, and the timeline
/// says the same thing in the way it draws them: a dim single line, shut,
/// opening onto the whole of what was said only when the reader asks.
///
/// A row with nothing behind it is not a fold. An event carrying neither a
/// summary nor links would otherwise offer a disclosure triangle onto an empty
/// box, which is a worse answer than the plain line it has always been.
///
/// A tool call's row is completed in place when its answer arrives, so the head
/// gains a state mark and the summary gains the answer as a second line. The
/// preview is the FIRST line either way, which is what keeps the head still
/// under a reader watching the call run.
///
/// The row carries no label. "Claude Code called a tool Bas…" spends the line
/// announcing a category and truncates the only part worth reading, so the row
/// IS its content — the same line the collapsed run shows, through the same
/// `activityMeat` — with the kind on the icon for a reader who cannot see it,
/// and the mark riding the content it is a fact about.
function activityHtml(event, meta, agentLabel, foldedChildrenHtml = "") {
  const summary = String(event.summary || "").trim();
  const sequence = sequenceAttribute(event);
  const head = `<span class="thread-event-icon" role="img" aria-label="${esc(eventLabel(meta, agentLabel))}">${esc(meta.icon)}</span>
    <span class="thread-activity-preview">${esc(activityMeat(event, meta, agentLabel))}</span>
    ${toolOutcomeHtml(event.outcome)}
    ${timeHtml(event.created_at)}`;
  // renderMarkdown escapes all input before adding its fixed safe tag set.
  const body = `${summary ? `<div class="thread-event-detail">${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ renderMarkdown(summary)}</div>` : ""}${linksHtml(event.links)}${foldedChildrenHtml}`;
  if (!body) return `<div class="thread-event thread-activity"${sequence}>${head}</div>`;
  return `<details class="thread-event thread-activity"${sequence}>
    <summary class="thread-activity-head">${head}</summary>
    ${body}
  </details>`;
}

function sequenceAttribute(event) {
  return Number.isFinite(event.sequence) ? ` data-sequence="${esc(event.sequence)}"` : "";
}

/// The activity meta for an event kind, or nothing for a kind that is not
/// activity.
///
/// A kind this client has never heard of is deliberately NOT activity: it
/// renders as the plain row it always did, and it ends a run rather than being
/// swept into one. The five kinds are a closed set the daemon and this client
/// agree on, and a row nobody can classify is better read as something that
/// happened than hidden inside a fold.
const activityMetaOf = (event) => {
  const meta = EVENT_META[event.event];
  return meta && meta.activity ? meta : null;
};

/// The line an activity row shows: the first line of what the agent actually
/// did, with no label in front of it. An item that carried no summary at all
/// has only its label to show, and showing the label is better than showing a
/// blank line.
///
/// One implementation for both places a row's line is drawn — the row's own
/// head and the collapsed run's — so an open run and the line that folds it can
/// never disagree about what a row says.
function activityMeat(event, meta, agentLabel) {
  const summary = String(event.summary || "").trim();
  return summary ? firstLine(summary) : eventLabel(meta, agentLabel);
}

/// How a folded run names itself in the document: the sequence it starts at,
/// and the newest sequence it stands for. Together they are the whole of what a
/// shut run says about what is inside it.
const ACTIVITY_RUN_ATTRIBUTE = "data-activity-run";
const ACTIVITY_RUN_FROM_ATTRIBUTE = "data-activity-from";
const ACTIVITY_RUN_THROUGH_ATTRIBUTE = "data-activity-through";

/// The run a press landed on, or nothing at all: a folded run's head is the
/// only thing in a timeline that opens one, and opening one is the pane's to
/// do — it can mean fetching what the run holds.
export function pressedActivityRunKey(target) {
  const head = target && target.closest ? target.closest(".thread-activity-group-head") : null;
  const run = head && head.parentElement;
  return run && run.hasAttribute(ACTIVITY_RUN_ATTRIBUTE) ? run.getAttribute(ACTIVITY_RUN_ATTRIBUTE) : null;
}

const runSpanAttribute = (run, name) => Number(run.getAttribute(name));

/// The run a sequence is folded into, or nothing when the row stands on its
/// own. A reference from elsewhere in the app (a subagent row naming the call
/// that spawned it) points at a sequence, and a shut run holds no row to point
/// at — so the run is opened first, and this is what says which one.
///
/// The span it matches on is the DIGEST's, which reaches back over the half of
/// a run the page cut away; the key it answers with is the run's, which is the
/// oldest sequence the window holds. Matching on the key instead would miss
/// every call that never travelled — exactly the ones a press has to fetch.
export function activityRunKeyAt(scroller, sequence) {
  const wanted = Number(sequence);
  if (!scroller || !Number.isFinite(wanted)) return null;
  const run = [...scroller.querySelectorAll(`[${ACTIVITY_RUN_ATTRIBUTE}]`)].find(
    (element) =>
      runSpanAttribute(element, ACTIVITY_RUN_FROM_ATTRIBUTE) <= wanted &&
      runSpanAttribute(element, ACTIVITY_RUN_THROUGH_ATTRIBUTE) >= wanted,
  );
  return run ? run.getAttribute(ACTIVITY_RUN_ATTRIBUTE) : null;
}

/// A run of activity, collapsed to one line.
///
/// Everything between two things somebody SAID is one row here: how many tools
/// the run called, and the last call's line, mark, time and glyph. The number is
/// the bridge's own count of the whole run rather than a count of the rows in
/// hand, so it says the same thing under a page that shipped a hundred of a
/// thousand calls as under one that shipped them all. No label — the reader can
/// see it is activity, and the label would spend the width of the line saying so
/// — and no tone, because a run of work asks for nothing. For a live run the
/// line is a ticker: each repaint shows the newest call and the count going up.
///
/// A run that called no tool at all keeps the old look: its latest row's words
/// and glyph, and how many rows it holds. Which of the two readings a run gets,
/// and where each printed value came from, is activityRunSummary's to resolve
/// (core/activityDigest.js); this prints what it was handed.
///
/// `children` is the html of what the run stands for, and null while the run is
/// shut — a shut run is a HEAD, and a thousand calls nobody has asked to see
/// are a thousand rows the document never has to hold. Which runs are open is
/// the pane's to say (core/activityRuns.js), because opening one can mean
/// fetching it.
///
/// Keyed by the run's FIRST item, so a run that grows under a reader watching it
/// keeps its identity — and with it, the scroll position inside the box they
/// opened. It carries the whole span it stands for as well — the digest's, so
/// the half a page cut away is inside it — which is how a reference from
/// somewhere else in the app finds the run a call is folded into without
/// opening every one of them (`activityRunKeyAt`).
///
/// The fold is the render's to write: `RENDERED_FOLD_ATTRIBUTE` tells the patch
/// so, and the pane cancels the press's own activation, so `open` says what the
/// pane says and a shut run is one nothing is drawn inside.
function activityRunHtml(span, summary, children) {
  return `<details class="thread-activity-group" ${RENDERED_FOLD_ATTRIBUTE} ${ACTIVITY_RUN_ATTRIBUTE}="${esc(String(span.key))}" ${ACTIVITY_RUN_FROM_ATTRIBUTE}="${esc(String(span.from))}" ${ACTIVITY_RUN_THROUGH_ATTRIBUTE}="${esc(String(span.through))}"${children === null ? "" : " open"}>
    <summary class="thread-activity-head thread-activity-group-head">
      <span class="thread-event-icon" aria-hidden="true">${esc(summary.icon)}</span>
      <span class="thread-activity-count">${summary.count}</span>
      <span class="thread-activity-preview">${esc(summary.meat)}</span>
      ${toolOutcomeHtml(summary.outcome)}
      ${timeHtml(summary.createdAt)}
    </summary>
    ${children === null ? "" : `<div class="thread-activity-group-list">${children}</div>`}
  </details>`;
}

/// The newest sequence a run stands for: its own rows, and the calls they fold.
const runThroughSequence = (run) =>
  run.reduce(
    (newest, row) =>
      (row.activity.toolCalls || []).reduce(
        (highest, call) => Math.max(highest, call.sequence || 0),
        Math.max(newest, row.activity.sequence || 0),
      ),
    0,
  );

/// What an open run draws: the items fetched for it, and whatever the window
/// holds past them.
///
/// A run older than the newest message never changes, so the fetched items are
/// the whole of it. The tail run is the one still being written, and the window
/// is where its newest rows land — so the rows past the fetch are taken from
/// the window, already rendered, and the head of a live run goes on ticking.
function runChildrenHtml(run, view) {
  const fetched = view.runItemsOf(run[0].key);
  if (!fetched || !fetched.length) return run.map((row) => row.html).join("");
  const fetchedThrough = fetched.reduce((newest, item) => Math.max(newest, item.data?.sequence || 0), 0);
  const live = run.filter((row) => Number(row.key) > fetchedThrough);
  return [...timelineRowsOf(fetched, view.agentLabel, view.threadId), ...live].map((row) => row.html).join("");
}

/// Where a run starts and where it reaches. The start is the digest's, because
/// a page that cut the run shipped only its newest rows and the calls before
/// them are still findable; the end is what the window holds, which for the
/// live tail run is newer than any digest.
function runSpan(run, digests) {
  const key = run[0].key;
  const digest = digestCovering(digests, Number(key));
  return { key, from: digest ? digest.from_sequence : key, through: runThroughSequence(run) };
}

function runEntry(run, digests, view) {
  const key = run[0].key;
  const children = view.openRuns.has(key) ? runChildrenHtml(run, view) : null;
  return { key, html: activityRunHtml(runSpan(run, digests), activityRunSummary(digests, run), children) };
}

/// Fold every maximal run of consecutive activity into one entry apiece, and
/// leave everything else exactly where it was.
function foldActivityRuns(rows, digests, view) {
  const entries = [];
  let run = [];
  const closeRun = () => {
    if (!run.length) return;
    entries.push(runEntry(run, digests, view));
    run = [];
  };
  for (const row of rows) {
    if (row.activity) {
      run.push(row);
      continue;
    }
    closeRun();
    entries.push({ key: row.key, html: row.html });
  }
  closeRun();
  return entries;
}

// eslint-disable-next-line complexity -- ratchet: eventHtml is at 11, cap 10 — reduce it, then drop this line
function eventHtml(event, agentLabel = "Agent", foldedChildrenHtml = "") {
  const meta = EVENT_META[event.event] || { label: String(event.event || "event").replaceAll("_", " "), icon: "•" };
  if (meta.activity) return activityHtml(event, meta, agentLabel, foldedChildrenHtml);
  const label = eventLabel(meta, agentLabel);
  const detail = event.revision_id
    ? `<button class="thread-revision-link" data-revision="${esc(event.revision_id)}">${esc(event.revision_id)}</button>`
    : event.event !== "done" && event.summary ? renderMarkdown(event.summary) : "";
  return `<div class="thread-event ${meta.tone || ""}">
    <span class="thread-event-icon" aria-hidden="true">${esc(meta.icon)}</span>
    <div class="thread-event-content"><div><strong>${esc(label)}</strong> ${timeHtml(event.created_at)}</div>${detail ? `<div class="thread-event-detail">${detail}</div>` : ""}${completionReportHtml(event.completion_report)}${linksHtml(event.links)}</div>
  </div>`;
}

const TOOL_CALL_KIND = "tool_use";

/// How a run of activity folds: which rows hide under which, and what calls a
/// row stands for.
///
/// A tool call that spawns a subagent owns everything the subagent did — those
/// rows are drawn inside its fold rather than beside it — so the row a reader
/// sees is one row and several calls. Both readings come from the same parent
/// map: the html of what folds under a row, and the calls that row stands for.
function threadFolding(items, agentLabel) {
  const eventItems = items.filter((item) => item.type !== "message");
  const sequenceOf = (item) => (item.data || {}).sequence;
  const parentSequenceOf = (item) => (item.data || {}).parent_sequence;
  const activitySequences = new Set(
    eventItems
      .filter((item) => activityMetaOf(item.data || {}))
      .map(sequenceOf)
      .filter((sequence) => Number.isFinite(sequence)),
  );
  const foldingChildren = eventItems.filter((item) => activitySequences.has(parentSequenceOf(item)));
  const foldedItems = new Set(foldingChildren);
  const childrenByParent = new Map();
  for (const item of foldingChildren) {
    const parent = parentSequenceOf(item);
    childrenByParent.set(parent, [...(childrenByParent.get(parent) || []), item]);
  }
  const foldedChildrenHtmlOf = (sequence, alreadyDrawn = new Set()) => {
    const children = childrenByParent.get(sequence);
    if (!children || alreadyDrawn.has(sequence)) return "";
    const drawn = new Set([...alreadyDrawn, sequence]);
    return `<div class="thread-activity-children">${children
      .map((child) => eventHtml(child.data || {}, agentLabel, foldedChildrenHtmlOf(sequenceOf(child), drawn)))
      .join("")}</div>`;
  };
  const toolCallsBeneath = (item, alreadyWalked) => {
    const sequence = sequenceOf(item);
    if (alreadyWalked.has(sequence)) return [];
    const walked = new Set([...alreadyWalked, sequence]);
    const event = item.data || {};
    const own = event.event === TOOL_CALL_KIND
      ? [{
        sequence,
        meat: activityMeat(event, EVENT_META[TOOL_CALL_KIND], agentLabel),
        outcome: event.outcome,
        createdAt: event.created_at,
      }]
      : [];
    const children = childrenByParent.get(sequence) || [];
    return [...own, ...children.flatMap((child) => toolCallsBeneath(child, walked))];
  };
  const toolCallsUnder = (item) => toolCallsBeneath(item, new Set());
  return { foldedItems, foldedChildrenHtmlOf, toolCallsUnder };
}

export function revealThreadSequence(scroller, sequence) {
  const wanted = Number(sequence);
  if (!scroller || !Number.isFinite(wanted)) return false;
  const row = scroller.querySelector(`[data-sequence="${wanted}"]`);
  if (!row) return false;
  for (let node = row; node && node !== scroller; node = node.parentElement) {
    if (node.tagName === "DETAILS") node.open = true;
  }
  if (row.scrollIntoView) row.scrollIntoView({ behavior: "smooth", block: "center" });
  return true;
}

/// One row of the timeline, before the runs are folded: its key, the item it
/// was drawn from, its html, and — for activity — what the fold reads off it.
///
/// Keyed by the item's own sequence, which is what makes a row's identity (and
/// a run's, which is its first row's) stable while the conversation grows. A
/// conversation rendered without sequences (the tests, and the initial-message
/// row) falls back to where the item sits.
const rowKey = (data, index) => String(data.sequence ?? `at-${index}`);

function activityRow(item, index, agentLabel, folding) {
  const event = item.data || {};
  const meta = activityMetaOf(event);
  const row = {
    key: rowKey(event, index),
    item,
    html: eventHtml(event, agentLabel, folding.foldedChildrenHtmlOf(event.sequence)),
  };
  if (!meta) return row;
  return {
    ...row,
    activity: {
      icon: meta.icon,
      sequence: event.sequence,
      meat: activityMeat(event, meta, agentLabel),
      outcome: event.outcome,
      createdAt: event.created_at,
      // What this row stands for, which is not always itself: a call that
      // spawned a subagent stands for every call the subagent made.
      toolCalls: folding.toolCallsUnder(item),
    },
  };
}

/// A message's row, or no row at all for the two the timeline does not draw.
///
/// `spoken` is whether this is the last thing said, which is the whole of
/// whether its offer can still be answered.
function messageRow(item, index, agentLabel, { threadId, spoken }) {
  const message = item.data || {};
  // Old bridges persisted the noisy structured handoff as a chat message.
  if (message.source === "completion" && String(message.body || "").includes("Completion report")) return [];
  // A choice is drawn on the chips that offered it, so the message it sent
  // would be the same words a second time.
  if (message.answers_options_of) return [];
  const key = offerKey(threadId, message.id);
  const live = spoken && !sendingChoices.has(key);
  return [{ key: rowKey(message, index), item, html: messageHtml(message, agentLabel, live, key) }];
}

/// Every top-level row a set of items draws, in order.
///
/// The one reading of what a row is: startup noise is not one, a row folded
/// under the call that spawned it is not one of its own, and two kinds of
/// message are drawn on other rows instead. Used for the conversation itself
/// and for the children of an open run, so a fetched run's rows are the rows
/// the window would have drawn for the same items.
function timelineRowsOf(sourceItems, agentLabel, threadId) {
  const items = sourceItems.filter((item) => !isStartupEvent(item));
  const folding = threadFolding(items, agentLabel);
  const topLevelItems = items.filter((item) => !folding.foldedItems.has(item));
  // Which message may still be answered with a chip: the last one said, and
  // only that one. An event between it and now changes nothing — a commit
  // landing is not somebody speaking.
  const lastSpoken = topLevelItems.reduce((last, item, index) => (item.type === "message" ? index : last), -1);
  return topLevelItems.flatMap((item, index) =>
    item.type === "message"
      ? messageRow(item, index, agentLabel, { threadId, spoken: index === lastSpoken })
      : [activityRow(item, index, agentLabel, folding)],
  );
}

/// The timeline: what was said, and what happened, as keyed entries.
///
/// Working time and the diffstat are NOT here. They are facts about the branch
/// or issue rather than about anything anyone said, they are true wherever you
/// are standing in the work, and they change every second — so they live on the
/// toolbar (core/toolbar.js) and the conversation keeps its own record: the
/// messages, the events, and whether the agent has read you.
///
/// One entry per top-level row, keyed so the reconciler can leave a row nobody
/// changed alone: a message under its sequence, a folded run under the sequence
/// it starts at, a lifecycle event under its own. `openRuns` says which runs
/// draw what they stand for, and `runItemsOf(key)` answers the items fetched
/// for one — both the pane's (core/activityRuns.js).
///
/// `itemCount` is how many conversation items the entries were drawn from,
/// which is not how many entries there are: a run of activity is many items
/// and one row, and the count on the conversation's title counts what was said
/// and done rather than how it fell into runs.
export function timelineEntries(sourceItems, agentLabel, threadId, digests, { openRuns, runItemsOf } = {}) {
  const rows = timelineRowsOf(sourceItems, agentLabel, threadId);
  const view = {
    agentLabel,
    threadId,
    openRuns: openRuns || NO_RUNS_OPEN,
    runItemsOf: runItemsOf || noRunItems,
  };
  return { entries: foldActivityRuns(rows, digests, view), itemCount: rows.length };
}

const NO_RUNS_OPEN = new Set();
const noRunItems = () => undefined;

// The plan composer's historical ids/copy, kept as the `composer: true`
// defaults so existing callers are unchanged.
const PLAN_COMPOSER_DEFAULTS = {
  inputId: "planthreadinput",
  sendId: "planthreadsend",
  hintId: "planthreadhint",
  placeholder: "Send a message to the planning agent…",
};

// `composer` is falsy (no composer), `true` (plan defaults), or an object
// overriding ids/placeholder/attachable — caller-scoped ids let two thread
// surfaces (the plan review and the run diff) each mount a composer on one page
// without colliding.
function threadComposerHtml(composer) {
  if (!composer) return "";
  return composerHtml({
    ...PLAN_COMPOSER_DEFAULTS,
    ...(composer === true ? {} : composer),
  });
}

/// The conversation's own status + lifecycle strip.
///
/// The surface bar carries tabs and a branch and nothing else, so where a run or
/// an issue STANDS, and what you can do about it, belong to the thread that
/// records how it got there: the chip rides the conversation's title, and the
/// verbs sit at the end of the timeline, right above the box you would reply in.
function statusChipHtml(status) {
  if (!status || !status.label) return "";
  return `<span class="chip thread-state ${esc(status.cls || "")}">${esc(status.label)}</span>`;
}

function threadActionsHtml(actionsId) {
  return actionsId ? `<div class="thread-actions" id="${esc(actionsId)}"></div>` : "";
}

/// What the daemon said each activity run over this window totals. A thread
/// rendered straight off the wire (a first paint, a test) carries none, and a
/// run with no digest counts what is in hand.
const digestsOf = (thread) => (thread && thread.activityDigests) || [];

/// The rows a timeline holds: its entries, or the one row a conversation with
/// nothing on the record shows. Keyed like any other, so the reconciler takes
/// it away the moment there is something to say.
const EMPTY_TIMELINE_ENTRY = { key: "empty", html: '<div class="thread-empty">No conversation yet.</div>' };

const timelineRows = ({ entries, itemCount }) => (itemCount ? entries : [EMPTY_TIMELINE_ENTRY]);

/// The conversation's own head: what it is, how much of it there is, and where
/// the work it records stands.
const threadTitleHtml = (itemCount, status) =>
  `<span class="thread-title-text">Conversation${itemCount ? ` <span>${itemCount}</span>` : ""}</span>${statusChipHtml(status)}`;

/// The initial message a surface opened the conversation with, ahead of the
/// items, until the conversation itself holds it.
function itemsWithInitialMessage(sourceItems, initialMessage) {
  const body = String(initialMessage || "").trim();
  const alreadySaid = sourceItems.some(
    (item) => item.type === "message" && item.data?.role === "user" && String(item.data.body || "").trim() === body,
  );
  if (!body || alreadySaid) return sourceItems;
  return [{ type: "message", data: { role: "user", body, seen_at: "initial" } }, ...sourceItems];
}

export function threadHtml(thread, options = {}) {
  const agentLabel = harnessLabel(thread, options.agentLabel);
  const items = itemsWithInitialMessage((thread && thread.items) || [], options.initialMessage);
  const built = timelineEntries(items, agentLabel, thread && thread.id, digestsOf(thread), options);
  // The timeline draws the avatar spine, and the messages sit in the gutter it
  // runs down. With nothing on the record there is neither, so the empty case
  // says so and the CSS drops both rather than ruling a line beside a sentence.
  const empty = built.itemCount ? "" : " is-empty";
  return `<section class="review-thread pane-col${empty}">
    <div class="thread-title">${threadTitleHtml(built.itemCount, options.status)}</div>
    <div class="thread-items thread-timeline${empty}">${timelineRows(built).map((row) => row.html).join("")}</div>
    <div class="thread-revision-view" hidden></div>
    ${threadActionsHtml(options.actionsId)}
    ${threadComposerHtml(options.composer)}
  </section>`;
}

/// What the paint is drawn from, as one string.
///
/// The conversation repaints on a poll and on every event, and most of those
/// ticks resolve exactly what the last one did. This is the one place the
/// inputs of a paint are named, so a tick that moved none of them can build
/// nothing at all: the window's delivery point and how many items it holds,
/// what the daemon said each run totals, which runs are open and which of them
/// have their items in hand, whose conversation it is and what that agent is
/// called, and the offer state riding the last message.
export function chatPaintFingerprint({
  deliveredSequence,
  itemCount,
  digests,
  openRunKeys,
  fetchedRunKeys,
  selectedAgentId,
  agentLabel,
  sending,
  choiceState,
}) {
  return [
    deliveredSequence,
    itemCount,
    digestsSignature(digests),
    keysSignature(openRunKeys),
    keysSignature(fetchedRunKeys),
    selectedAgentId || "",
    agentLabel || "",
    sending || "",
    choiceState || "",
  ].join("|");
}

const lastCallSignature = (call) => (call ? `${call.sequence}:${call.outcome || ""}` : "");

const digestsSignature = (digests) =>
  (digests || [])
    .map((digest) => `${digest.from_sequence}-${digest.through_sequence}:${digest.tool_calls}:${lastCallSignature(digest.last_tool_call)}`)
    .join(",");

const keysSignature = (keys) => [...(keys || [])].sort().join(",");

/// The offers in hand as the paint sees them: what has been picked and not yet
/// sent, and what is being sent right now. Both are this module's own state —
/// the chips write it and the timeline reads it — so a paint that skips has to
/// be able to see it move.
export function threadOfferState() {
  const picks = [...pendingChoices].map(([key, chosen]) => `${key}=${[...chosen].sort().join(",")}`);
  return { choiceState: picks.sort().join("|"), sending: [...sendingChoices].sort().join("|") };
}

const NOTHING_SAID_YET = { items: [] };

/// Whether the conversation is drawn as one that has nothing on the record —
/// the timeline draws the avatar spine, and with nothing to hang on it there is
/// neither spine nor gutter.
function showEmptyThread(section, empty) {
  section.classList.toggle("is-empty", empty);
  section.querySelector(".thread-items").classList.toggle("is-empty", empty);
}

/// Draw a conversation into `container` as keyed rows.
///
/// The frame — the title, the timeline, the revision viewer, whatever composer
/// the caller asked for — is written once, from the same builder `threadHtml`
/// is; the rows inside it are the reconciler's from then on, so a row nobody
/// changed keeps its element and everything the browser hangs off it: the
/// selection in it, the fold the reader opened, the picture it had fetched, and
/// the place scroll anchoring was holding.
///
/// Answers whether the frame was written this time, which is when the caller
/// has to wire what is in it.
export function paintThreadEntries(container, built, options = {}) {
  const framed = !container.querySelector(".thread-items");
  if (framed) {
    writeThreadKeepingComposer(container, threadHtml(NOTHING_SAID_YET, options));
    container.querySelector(".thread-items").replaceChildren();
  }
  const section = container.querySelector(".review-thread");
  patchInnerHtml(section.querySelector(".thread-title"), threadTitleHtml(built.itemCount, options.status));
  showEmptyThread(section, !built.itemCount);
  patchList(section.querySelector(".thread-items"), timelineRows(built), {
    keyOf: (row) => row.key,
    render: (row) => row.html,
  });
  return framed;
}

/// The parts of a rendered thread a repaint may overwrite. The composer is
/// deliberately not among them, and neither is the revision viewer — both are
/// state the reader put there, and the poll knows nothing about either.
const REPAINTED_PARTS = [".thread-title", ".thread-items", ".thread-actions"];

/// What makes two composers the same box: the input it writes into, and
/// whether it takes files. Anything else about it (the placeholder) is moved
/// onto the live one rather than rebuilt.
const composerSignature = (composer) => {
  const input = composer.querySelector("textarea");
  return `${input ? input.id : ""}:${composer.querySelector(".composer.attachable") ? "files" : "text"}`;
};

/// Whether the live composer can stay: the same box on both sides, or no box on
/// either — a surface that pins its composer OUTSIDE the thread (the agent
/// rail) renders none here, and two renders that both carry none agree.
const composerSurvives = (live, next) => {
  const liveComposer = live.querySelector(".thread-composer");
  const nextComposer = next.querySelector(".thread-composer");
  if (!liveComposer || !nextComposer) return !liveComposer && !nextComposer;
  return composerSignature(liveComposer) === composerSignature(nextComposer);
};

/// Write a freshly rendered thread into `container`, keeping what is live.
///
/// Every conversation surface re-renders the whole section on its poll, and
/// writing that string in replaces every element under it. Two things cannot
/// survive that. A composer inside the container loses the words, the caret and
/// the FOCUS — on a touch device the software keyboard opens and then shuts a
/// tick and a half later, so a message cannot be typed at all. And a timeline
/// that says exactly what it said a tick ago collapses any selection being made
/// in it and sends every inline image back for a re-fetch.
///
/// So only the parts that actually changed are written, and a composer the
/// render carries is matched against the live one rather than replaced:
/// everything the browser hangs off it (focus, selection, the keyboard, the
/// IME's composition) simply stays.
///
/// Returns whether the container was rewritten wholesale. True means the caller
/// must wire everything in it again, including any composer this render
/// created; false means the wired one is still there and re-wiring it would
/// throw away the tray's uploads.
// eslint-disable-next-line complexity -- ratchet: writeThreadKeepingComposer is at 11, cap 10 — reduce it, then drop this line
export function writeThreadKeepingComposer(container, html) {
  const live = container.querySelector(".review-thread");
  const rendered = container.ownerDocument.createElement("div");
  rendered.innerHTML = html;
  const next = rendered.querySelector(".review-thread");
  if (!live || !next || !composerSurvives(live, next)) {
    container.innerHTML = html;
    return true;
  }
  if (live.className !== next.className) live.className = next.className;
  for (const selector of REPAINTED_PARTS) {
    const target = live.querySelector(selector);
    const source = next.querySelector(selector);
    if (!target || !source) continue;
    // Most ticks resolve the same conversation, and the ones that do not
    // usually change a single message — the newest, while an agent writes it.
    // Writing the whole part in would collapse a selection being made in a
    // message, send every inline image back for a re-fetch, and leave the
    // browser's scroll anchoring with nothing to hold on to under a reader who
    // is mid-scroll. So the two are walked together and only the words that
    // actually changed are written; a part that says what it already said is
    // left untouched down to the last attribute.
    patchElement(target, source);
  }
  const liveInput = live.querySelector(".thread-composer textarea");
  const nextInput = next.querySelector(".thread-composer textarea");
  if (liveInput && nextInput && liveInput.placeholder !== nextInput.placeholder) {
    liveInput.placeholder = nextInput.placeholder;
  }
  return false;
}

export function paintThreadKeepingPlace(scroller, paint, { olderItemsPrepended = false } = {}) {
  paintKeepingPlace(scroller, paint, {
    opening: (element) => !element.querySelector(".review-thread"),
    policy: pinToBottom({ olderItemsPrepended }),
  });
}

export function wireThreadRevisionLinks(root, loadRevision) {
  if (!root) return;
  root.querySelectorAll(".thread-revision-link").forEach((button) => {
    button.onclick = async () => {
      const thread = button.closest(".review-thread");
      const viewer = thread && thread.querySelector(".thread-revision-view");
      if (!viewer) return;
      viewer.hidden = false;
      viewer.innerHTML = `<div class="thread-revision-head">Loading ${esc(button.dataset.revision)}…</div>`;
      try {
        const revision = await loadRevision(button.dataset.revision);
        viewer.innerHTML = `<div class="thread-revision-head"><strong>${esc(revision.revision_id)}</strong><button type="button" aria-label="Close revision">×</button></div><pre>${esc(revision.contents || "")}</pre>`;
        viewer.querySelector("button").onclick = () => {
          viewer.hidden = true;
          viewer.innerHTML = "";
        };
      } catch (error) {
        viewer.innerHTML = `<div class="thread-revision-head">Could not load revision: ${esc(error.message || String(error))}</div>`;
      }
    };
  });
}

/// Fill the images a rendered timeline is waiting on, and make the file chips
/// download what they name.
///
/// `load(path)` resolves the bridge's `thread.attachment` payload
/// (`{mime, content_b64}`).
/// Wire the suggested actions: the picking, and the one press that sends them.
///
/// `submit` takes `{ messageId, optionIds }` and answers with a promise. The
/// offer shuts the instant it is pressed rather than when the daemon answers —
/// the reader has made their choice, and a second press would send it twice —
/// and comes back if the send is refused, with what they picked still marked.
export function wireThreadOptions(root, submit) {
  if (!root) return;
  root.querySelectorAll(".thread-options").forEach((group) => {
    const messageId = group.dataset.message;
    const key = group.dataset.offer;
    const chips = [...group.querySelectorAll(".thread-option")];
    const send = group.querySelector(".thread-options-send");
    const picked = () => chips.filter((chip) => chip.classList.contains("chosen")).map((chip) => chip.dataset.optionId);
    const shut = (closed) => {
      chips.forEach((chip) => {
        chip.disabled = closed;
      });
      if (send) send.disabled = closed || !picked().length;
    };

    chips.forEach((chip) => {
      chip.onclick = () => {
        if (chip.disabled) return;
        const chosen = !chip.classList.contains("chosen");
        chip.classList.toggle("chosen", chosen);
        chip.setAttribute("aria-pressed", String(chosen));
        // Remembered outside the markup, so the next repaint of the
        // conversation draws the selection back rather than clearing it.
        pendingChoices.set(key, new Set(picked()));
        if (send) send.disabled = !picked().length;
      };
    });

    if (!send) return;
    send.onclick = () => {
      const optionIds = picked();
      if (!optionIds.length || send.disabled) return;
      sendingChoices.add(key);
      shut(true);
      Promise.resolve(submit({ messageId, optionIds })).then(
        () => {
          sendingChoices.delete(key);
          pendingChoices.delete(key);
        },
        () => {
          // Refused: the offer is still the last thing said, so it goes back
          // to being answerable with the same chips still marked.
          sendingChoices.delete(key);
          shut(false);
        },
      );
    };
  });
}

export function wireThreadAttachments(root, load) {
  if (!root) return;
  const dataUrlFor = async (path) => {
    const held = attachmentDataUrls.get(path);
    if (held) return held;
    const attachment = await load(path);
    const dataUrl = `data:${attachment.mime || "application/octet-stream"};base64,${attachment.content_b64 || ""}`;
    rememberAttachment(path, dataUrl);
    return dataUrl;
  };

  root.querySelectorAll("img.thread-attachment-image").forEach((image) => {
    const path = image.dataset.attachmentPath;
    // A picture already showing, and one already asked for and refused, are
    // both settled: asking again on every poll would be a request a second and
    // a half for bytes the reader is not going to get.
    if (!path || image.getAttribute("src") || attachmentDataUrls.get(path) === null) return;
    dataUrlFor(path).then(
      (dataUrl) => {
        image.setAttribute("src", dataUrl);
      },
      () => {
        // A picture that will not load says so where the picture would be,
        // rather than leaving a silent gap in the conversation — remembered as
        // well as shown, so the next render draws the same unavailable figure.
        rememberAttachment(path, null);
        image.closest(".thread-attachment-figure")?.classList.add("unavailable");
      },
    );
  });

  root.querySelectorAll("button.thread-attachment").forEach((chip) => {
    chip.onclick = async () => {
      const path = chip.dataset.attachmentPath;
      if (!path) return;
      const dataUrl = await dataUrlFor(path);
      const link = root.ownerDocument.createElement("a");
      link.href = dataUrl;
      link.download = chip.dataset.attachmentName || "attachment";
      link.click();
    };
  });
}

export function wireThreadLinks(root, openLink) {
  if (!root) return;
  root.querySelectorAll(".thread-reference").forEach((button) => {
    // eslint-disable-next-line complexity -- ratchet: this callback is at 12, cap 10 — reduce it, then drop this line
    button.onclick = () => {
      const link = { kind: button.dataset.kind };
      if (button.dataset.path) link.path = button.dataset.path;
      if (button.dataset.issueId) link.issue_id = button.dataset.issueId;
      if (button.dataset.planId) link.plan_id = button.dataset.planId;
      if (button.dataset.stageId) link.stage_id = button.dataset.stageId;
      if (button.dataset.implementationId) link.implementation_id = button.dataset.implementationId;
      if (button.dataset.runId) link.run_id = button.dataset.runId;
      if (button.dataset.worktreeId) link.worktree_id = button.dataset.worktreeId;
      if (button.dataset.sha) link.sha = button.dataset.sha;
      if (button.dataset.recoveryId) link.recovery_id = button.dataset.recoveryId;
      if (button.dataset.lineStart) link.line_start = Number(button.dataset.lineStart);
      if (button.dataset.lineEnd) link.line_end = Number(button.dataset.lineEnd);
      openLink(link);
    };
  });
}

/// Wire a thread composer's submit path: draft restore, re-entry guard,
/// in-place button restore, and Cmd/Ctrl+Enter.
///
/// Shared because the plan and diff composers are the same gesture and drifted
/// apart once: Cmd+Enter reaches submit without the button's native disabled
/// gate, so a composer that only disables the button double-posts on the retry
/// a wedged-looking box invites. Restoring the button here — before any
/// repaint — keeps that true even when the caller's rebuild is frozen.
///
/// `onSubmit(body, attachments, { interrupt })` does the transport —
/// `interrupt` is true only where the send control offered the alternative and
/// the writer chose it. `upload` (with the
/// `readAttachments`/`writeAttachments` draft pair) turns the box into one that
/// takes files; without it the composer is the plain text box it always was.
///
/// Returns a controller: `setCanInterrupt(flag)` moves the send between its two
/// shapes in place, for a surface whose poll can change the answer under a box
/// somebody is typing in.
/// The send button's word. It wraps its label so a busy state can rewrite the
/// word without wiping the icon beside it; the split shape, which has no icon
/// to protect, is driven directly.
const sendLabel = (button) => button.querySelector(".composer-send-label") || button;

export function wireThreadComposer(root, { ids, onSubmit, readDraft, writeDraft, onError, afterSubmit, upload, readAttachments, writeAttachments }) {
  if (!root) return null;
  const input = root.querySelector(`#${ids.input}`);
  let send = root.querySelector(`#${ids.send}`);
  const control = root.querySelector(`#${composerPartIds(ids.input).sendControl}`);
  const hint = ids.hint ? root.querySelector(`#${ids.hint}`) : null;
  if (!input || !send) return null;

  const say = (message) => {
    if (hint) hint.textContent = message;
  };
  const tray = upload
    ? mountComposerAttachments(root, {
        ids,
        upload,
        readAttachments,
        writeAttachments,
        onError: say,
      })
    : null;

  input.value = readDraft();
  input.oninput = () => {
    writeDraft(input.value);
    say("");
  };
  const fitToText = autoGrow(input);

  /// The caret half of a split send, when the control is wearing that shape.
  const caretOf = () => control && control.querySelector(".caret");
  const setPressable = (pressable) => {
    send.disabled = !pressable;
    const caret = caretOf();
    if (caret) caret.disabled = !pressable;
  };

  // eslint-disable-next-line complexity -- ratchet: this callback is at 14, cap 10 — reduce it, then drop this line
  const submit = async ({ interrupt = false } = {}) => {
    // A send is already in flight: the keyboard path has no disabled gate.
    if (send.disabled) return;
    if (tray && tray.busy()) {
      // The message names its files by path, so posting before they land would
      // hand the agent a reference to bytes that do not exist yet.
      say("A file is still attaching…");
      return;
    }
    const body = input.value.trim();
    // A file on its own is a message; words are only required when there is
    // nothing else being sent.
    if (!body && (!tray || tray.isEmpty())) {
      say("Type a message first.");
      input.focus();
      return;
    }
    setPressable(false);
    sendLabel(send).textContent = "sending…";
    try {
      const result = await onSubmit(body, tray ? tray.attachments() : [], { interrupt });
      writeDraft("");
      input.value = "";
      fitToText();
      if (tray) tray.clear();
      setPressable(true);
      sendLabel(send).textContent = "Send";
      if (afterSubmit) afterSubmit(result);
    } catch (error) {
      // The text and the files stay put: a failed send must never cost the user
      // their words, and re-picking the files would be worse.
      setPressable(true);
      sendLabel(send).textContent = "Send";
      if (onError) onError(error);
    }
  };

  /// Take hold of whichever shape the send control is wearing. Called again
  /// after a swap, because the buttons it wires are new elements.
  const wireSendControl = () => {
    send = root.querySelector(`#${ids.send}`);
    if (!send) return;
    send.onclick = () => submit();
    if (control) {
      mountSplitMenu(control, { onChoose: (action) => submit({ interrupt: action === INTERRUPT_SEND_OPTION.id }) });
    }
  };

  /// Move the send between its two shapes. Never mid-press: a send in flight
  /// owns the button's word, and an open menu is a choice being made — the
  /// poll comes round again a second later, and by then the press has landed.
  let splitShown = !!(control && control.querySelector(".splitmenu"));
  const setCanInterrupt = (wanted) => {
    const split = !!wanted;
    if (!control || split === splitShown) return;
    if (send.disabled || control.querySelector(".splitmenu:not([hidden])")) return;
    control.innerHTML = sendControlHtml({ sendId: ids.send, canInterrupt: split });
    splitShown = split;
    wireSendControl();
  };

  wireSendControl();
  input.onkeydown = (event) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      submit();
    }
  };

  return { setCanInterrupt };
}

