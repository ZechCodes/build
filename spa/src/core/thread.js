import { esc } from "./text.js";
import { renderMarkdown } from "./markdown.js";
import { patchElement } from "./domPatch.js";
import { completionReportSections } from "./agentRailModel.js";
import {
  autoGrow,
  composerHtml,
  formatAttachmentSize,
  isImageAttachment,
  mountComposerAttachments,
} from "./composer.js";

const EVENT_META = {
  session_started: { label: "Agent session started", icon: "▶" },
  session_ended: { label: "Agent session ended", icon: "■" },
  run_started: { label: "Run started", icon: "▶" },
  run_failed: { label: "Agent reported failure", icon: "×", tone: "blocked" },
  blocked: { label: "Agent reported a blocker", icon: "!", tone: "blocked" },
  review_blocked: { label: "Review blocked", icon: "!", tone: "blocked" },
  idle_unreported: { label: "Agent went idle without reporting", icon: "…", tone: "blocked" },
  done: { label: "Agent reported done", icon: "✓", tone: "success" },
  revision_created: { label: "Revision created", icon: "↻" },
  approved: { label: "Issue ready", icon: "✓", tone: "success" },
  stage_approved: { label: "Stage approved", icon: "✓", tone: "success" },
  stage_started: { label: "Stage implementation started", icon: "▶" },
  implementation_started: { label: "Implementation started", icon: "▶" },
  worktree_reused: { label: "Implementation worktree reused", icon: "↻", tone: "success" },
  worktree_recreated: { label: "Implementation worktree recreated", icon: "↻", tone: "success" },
  worktree_deleted: { label: "Implementation worktree deleted", icon: "×", tone: "blocked" },
  recovery_started: { label: "Verified recovery started", icon: "▶" },
  recovery_succeeded: { label: "Verified recovery succeeded", icon: "✓", tone: "success" },
  recovery_failed: { label: "Verified recovery failed", icon: "×", tone: "blocked" },
  stage_completed: { label: "Stage completed", icon: "✓", tone: "success" },
  stage_invalidated: { label: "Stage marked incomplete", icon: "!", tone: "blocked" },
  implementation_archived: { label: "Implementation archived", icon: "■" },
  // A pass that only orders the diff for review: it carries no tone, because
  // it asks the reviewer for nothing.
  triaged: { label: "Diff ordered for review", icon: "≡" },
  // The reviewer disagreed with where the pass put something. Also toneless:
  // it is a note to the agent, not a call on anybody.
  triage_overridden: { label: "Review order corrected", icon: "≠" },
  committed: { label: "Changes committed", icon: "◆", tone: "success" },
  pushed: { label: "Changes pushed", icon: "↑", tone: "success" },
  merged: { label: "Changes merged", icon: "⌁", tone: "success" },
  abandoned: { label: "Abandoned", icon: "×", tone: "blocked" },
};

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
// dropped delta), resets it to a full refetch.
// How much conversation a first load asks for. The daemon clamps whatever it
// hears, so this is a request rather than a promise — but it has to be made:
// a poll that names no bound gets the conversation whole, which is the only
// answer a client written before paging could reconcile.
export const FIRST_PAGE_ITEMS = 60;

// What a MUTATION asks its answer to carry. Every mutation RPC answers with
// the whole entity, conversation included, and no caller here reads that
// answer — the refresh that follows is what paints. The page is asked for
// anyway: an answer nobody reads must still not grow with the conversation,
// and a call that names no bound gets every item it ever held. A cursor is no
// use here, since the answer has to stand on its own for whoever starts
// reading it.
export const MUTATION_THREAD_PAGE = Object.freeze({ thread_limit: FIRST_PAGE_ITEMS });

export function createThreadCache() {
  let accumulatedItems = [];
  // Whether the daemon said there is conversation above the window. Only a
  // paged answer knows; a forward delta says nothing about the far end.
  let olderItemsRemain = false;
  // The newest counter value the daemon has named that this cache has already
  // taken delivery of. Usually the top of the window — but an item mutated in
  // place BELOW the window is taken delivery of by being left out of it (see
  // `theWindowMayTake`), and the cursor still has to move past its bump or the
  // daemon re-ships it on every poll for as long as the view is open.
  let deliveredSequence = 0;

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
  const holdsAnUnbrokenRunEndingAtTheNewest = (merged, delivered, threadPayload) => {
    const newest = threadPayload.thread_last_sequence;
    // A daemon old enough not to name its newest sequence leaves only the size
    // check to go on.
    if (newest != null && delivered !== newest) return false;
    return merged.length <= threadPayload.thread_total;
  };

  return {
    // Extra params for the next plan.get / run.get: the last sequence held, or
    // — with no window open (first load, or after a reset) — how much of the
    // newest conversation to open one on.
    cursorParam() {
      return accumulatedItems.length
        ? { thread_after_sequence: deliveredSequence }
        : { thread_limit: FIRST_PAGE_ITEMS };
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
    // Fold a polled thread payload into the cache and return a thread whose
    // `items` is the complete accumulated list. Never mutates the payload.
    absorb(threadPayload) {
      if (!threadPayload) {
        accumulatedItems = [];
        olderItemsRemain = false;
        deliveredSequence = 0;
        return threadPayload;
      }
      const arrivedItems = threadPayload.items || [];
      // A paged answer is the only one that knows what lies above it; a bare
      // forward delta leaves the standing answer alone, and so does a page that
      // no longer speaks for the window's floor.
      if (threadPayload.has_more != null && speaksForTheWindowsFloor(arrivedItems)) {
        olderItemsRemain = threadPayload.has_more === true;
      }
      if (threadPayload.thread_total == null) {
        // An uncursored (full) response is authoritative: replace, don't merge.
        accumulatedItems = [...arrivedItems];
        deliveredSequence = highestCursorSequence(accumulatedItems);
        return { ...threadPayload, items: accumulatedItems };
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
      if (!accumulatedItems.length && threadPayload.has_more == null) {
        return { ...threadPayload, items: arrivedItems };
      }
      const merged = mergeArrivals(theWindowMayTake(arrivedItems));
      // Everything the delta carried is delivered, whether the window took it
      // or left it below the floor.
      const delivered = Math.max(
        deliveredSequence,
        highestCursorSequence(merged),
        highestCursorSequence(arrivedItems),
      );
      if (!holdsAnUnbrokenRunEndingAtTheNewest(merged, delivered, threadPayload)) {
        // A real gap: render what we have this tick, but drop the cache so the
        // next poll refetches from the newest page down and self-heals.
        accumulatedItems = [];
        olderItemsRemain = false;
        deliveredSequence = 0;
        return { ...threadPayload, items: merged };
      }
      accumulatedItems = merged;
      deliveredSequence = delivered;
      return { ...threadPayload, items: accumulatedItems };
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
      return { ...pagePayload, items: accumulatedItems };
    },
    reset() {
      accumulatedItems = [];
      olderItemsRemain = false;
      deliveredSequence = 0;
    },
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

function harnessLabel(thread, override) {
  const raw = override || (thread && thread.sessions && thread.sessions.at(-1)?.provider) || "Agent";
  if (raw === "codex" || raw === "Codex CLI") return "Codex";
  if (raw === "claude" || raw === "Claude") return "Claude Code";
  return raw;
}

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

function messageHtml(message, agentLabel = "Agent", liveOptions = false, offer = "") {
  const user = message.role === "user";
  const status = user
    ? `<span class="thread-status">${message.seen_at ? "Seen" : "Unread"}${message.resolved_by_revision ? ` · <button class="thread-revision-link" data-revision="${esc(message.resolved_by_revision)}">Resolved in ${esc(message.resolved_by_revision)}</button>` : ""}</span>`
    : "";
  // `done` is message metadata, not a presentation type. A done-flagged send
  // follows the timeline's done event and otherwise renders like every message.
  // renderMarkdown escapes all input before adding its fixed safe tag set.
  return `<article class="thread-message thread-comment ${user ? "user" : "agent"}">
    <span class="thread-avatar" aria-hidden="true">${user ? "Y" : "A"}</span>
    <div class="thread-comment-card">
      <div class="thread-message-head"><span><strong>${user ? "You" : esc(agentLabel)}</strong> commented ${timeHtml(message.created_at)}</span>${status}</div>
      ${anchorLabel(message.anchor)}
      ${message.body ? `<div class="thread-body markdown">${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ renderMarkdown(message.body)}</div>` : ""}
      ${attachmentsHtml(message.attachments)}
      ${linksHtml(message.links)}
      ${optionsHtml(message, liveOptions, offer)}
    </div>
  </article>`;
}

/// The agent's handoff, as a card.
///
/// `done` is asked for a completion report and the event is the whole record of
/// it (there is no companion message any more), so the report renders where the
/// event does: the critical files, the decisions a reviewer would otherwise
/// reverse-engineer, the risks, and what was deliberately left alone. Every
/// line is the agent's words — escaped.
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

function eventHtml(event, agentLabel = "Agent") {
  const meta = EVENT_META[event.event] || { label: String(event.event || "event").replaceAll("_", " "), icon: "•" };
  const label = meta.label.replace(/^Agent\b/, agentLabel);
  const detail = event.revision_id
    ? `<button class="thread-revision-link" data-revision="${esc(event.revision_id)}">${esc(event.revision_id)}</button>`
    : event.event !== "done" && event.summary ? renderMarkdown(event.summary) : "";
  return `<div class="thread-event ${meta.tone || ""}">
    <span class="thread-event-icon" aria-hidden="true">${esc(meta.icon)}</span>
    <div class="thread-event-content"><div><strong>${esc(label)}</strong> ${timeHtml(event.created_at)}</div>${detail ? `<div class="thread-event-detail">${detail}</div>` : ""}${completionReportHtml(event.completion_report)}${linksHtml(event.links)}</div>
  </div>`;
}

/// The timeline: what was said, and what happened.
///
/// Working time and the diffstat are NOT here. They are facts about the branch
/// or issue rather than about anything anyone said, they are true wherever you
/// are standing in the work, and they change every second — so they live on the
/// toolbar (core/toolbar.js) and the conversation keeps its own record: the
/// messages, the events, and whether the agent has read you.
function timelineHtml(items, agentLabel, threadId) {
  // Which message may still be answered with a chip: the last one said, and
  // only that one. An event between it and now changes nothing — a commit
  // landing is not somebody speaking.
  const lastSpoken = items.reduce((last, item, index) => (item.type === "message" ? index : last), -1);
  return items.flatMap((item, index) => {
    if (item.type !== "message") return [eventHtml(item.data || {}, agentLabel)];
    const message = item.data || {};
    // Old bridges persisted the noisy structured handoff as a chat message.
    if (message.source === "completion" && String(message.body || "").includes("Completion report")) return [];
    // A choice is drawn on the chips that offered it, so the message it sent
    // would be the same words a second time.
    if (message.answers_options_of) return [];
    const key = offerKey(threadId, message.id);
    const live = index === lastSpoken && !sendingChoices.has(key);
    return [messageHtml(message, agentLabel, live, key)];
  });
}

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

export function threadHtml(thread, options = {}) {
  const agentLabel = harnessLabel(thread, options.agentLabel);
  const sourceItems = (thread && thread.items) || [];
  const initialMessage = String(options.initialMessage || "").trim();
  const hasInitialMessage = sourceItems.some(
    (item) => item.type === "message" && item.data?.role === "user" && String(item.data.body || "").trim() === initialMessage,
  );
  const items = initialMessage && !hasInitialMessage
    ? [{ type: "message", data: { role: "user", body: initialMessage, seen_at: "initial" } }, ...sourceItems]
    : sourceItems;
  const renderedItems = timelineHtml(items, agentLabel, thread && thread.id);
  const itemCount = renderedItems.length;
  // The timeline draws the avatar spine, and the messages sit in the gutter it
  // runs down. With nothing on the record there is neither, so the empty case
  // says so and the CSS drops both rather than ruling a line beside a sentence.
  const empty = itemCount ? "" : " is-empty";
  return `<section class="review-thread pane-col${empty}">
    <div class="thread-title"><span class="thread-title-text">Conversation${itemCount ? ` <span>${itemCount}</span>` : ""}</span>${statusChipHtml(options.status)}</div>
    <div class="thread-items thread-timeline${empty}">${itemCount
      ? renderedItems.join("")
      : '<div class="thread-empty">No conversation yet.</div>'}</div>
    <div class="thread-revision-view" hidden></div>
    ${threadActionsHtml(options.actionsId)}
    ${threadComposerHtml(options.composer)}
  </section>`;
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

/** How near the end still counts as reading the end. Absorbs the fractional
 *  scroll heights a zoomed or sub-pixel layout leaves behind. */
const AT_BOTTOM_SLACK_PX = 32;

/// Run `paint` and report whether it moved anything under `scroller`.
///
/// Asking the DOM is the only honest answer: the paint belongs to the caller,
/// and a poll's repaint that resolved the same conversation writes nothing at
/// all. Observing it costs one observer per tick and tells the difference
/// between a repaint and a tick that merely happened.
function paintAndSayWhetherAnythingMoved(scroller, paint) {
  if (typeof MutationObserver !== "function") {
    paint();
    return true;
  }
  const observer = new MutationObserver(() => {});
  observer.observe(scroller, { childList: true, subtree: true, attributes: true, characterData: true });
  try {
    paint();
    return observer.takeRecords().length > 0;
  } finally {
    observer.disconnect();
  }
}

/// Paint a conversation with the reader's place kept.
///
/// The newest message is the one the human came for and it sits at the END, so
/// opening a thread lands at the bottom. Every surface then re-renders the whole
/// timeline on its poll, and writing innerHTML resets scrollTop — which is the
/// same lever, so both halves live here: a reader already at the end is carried
/// along with new messages, and a reader who scrolled up is left exactly where
/// they were rather than yanked back down mid-sentence.
///
/// `scroller` is the element that scrolls (the surfaces' `#tabbody`), which is
/// not always the element `paint` writes into — the issue surface paints a
/// wrapper inside it. With no scroller this is `paint()` and nothing else.
///
/// A tick whose paint wrote nothing is not a repaint, and the scroller is not
/// touched for it — not even to write back the number it already holds. That
/// assignment is not free: on iOS it cancels the momentum of a flick in
/// progress and drops the reader back where the tick found them, which at a
/// poll every 1.6 seconds is a thread that cannot be scrolled down at all.
///
/// `olderItemsPrepended` says this paint grew the timeline at the TOP — a page
/// of history the reader asked for by scrolling back past the start of the
/// window. Everything they were reading has moved down by the height of what
/// arrived, so keeping their scrollTop would keep the pixel and lose the
/// message, jumping them a page further back on every load.
export function paintThreadKeepingPlace(scroller, paint, { olderItemsPrepended = false } = {}) {
  if (!scroller) {
    paint();
    return;
  }
  // Nothing rendered yet means this paint is the open: the tab was just
  // selected, or a shell rebuild wiped the body under it.
  const opening = !scroller.querySelector(".review-thread");
  const previousScrollTop = scroller.scrollTop;
  const previousScrollHeight = scroller.scrollHeight;
  const wasAtBottom =
    previousScrollHeight - scroller.clientHeight - previousScrollTop <= AT_BOTTOM_SLACK_PX;
  const changed = paintAndSayWhetherAnythingMoved(scroller, paint);
  if (olderItemsPrepended) {
    scroller.scrollTop = previousScrollTop + (scroller.scrollHeight - previousScrollHeight);
    return;
  }
  if (!opening && !changed) return;
  if (!opening && !wasAtBottom) {
    scroller.scrollTop = previousScrollTop;
    return;
  }
  const toBottom = () => {
    scroller.scrollTop = scroller.scrollHeight;
  };
  toBottom();
  // Markdown and web fonts can settle a frame after the content lands, leaving
  // the open short of the newest message. Only the open re-pins: doing it on a
  // poll's repaint would fight a reader who scrolled away within that frame.
  if (opening && typeof requestAnimationFrame === "function") requestAnimationFrame(toBottom);
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
/// `onSubmit(body, attachments)` does the transport and resolves when the post
/// has landed. `upload` (with the `readAttachments`/`writeAttachments` draft
/// pair) turns the box into one that takes files; without it the composer is
/// the plain text box it always was.
/// The send button's word. It wraps its label so a busy state can rewrite the
/// word without wiping the icon beside it; an older composer without the span
/// is still driven directly.
const sendLabel = (button) => button.querySelector(".composer-send-label") || button;

export function wireThreadComposer(root, { ids, onSubmit, readDraft, writeDraft, onError, afterSubmit, upload, readAttachments, writeAttachments }) {
  if (!root) return;
  const input = root.querySelector(`#${ids.input}`);
  const send = root.querySelector(`#${ids.send}`);
  const hint = ids.hint ? root.querySelector(`#${ids.hint}`) : null;
  if (!input || !send) return;

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

  const submit = async () => {
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
    send.disabled = true;
    sendLabel(send).textContent = "sending…";
    try {
      const result = await onSubmit(body, tray ? tray.attachments() : []);
      writeDraft("");
      input.value = "";
      fitToText();
      if (tray) tray.clear();
      send.disabled = false;
      sendLabel(send).textContent = "Send";
      if (afterSubmit) afterSubmit(result);
    } catch (error) {
      // The text and the files stay put: a failed send must never cost the user
      // their words, and re-picking the files would be worse.
      send.disabled = false;
      sendLabel(send).textContent = "Send";
      if (onError) onError(error);
    }
  };

  send.onclick = submit;
  input.onkeydown = (event) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      submit();
    }
  };
}

