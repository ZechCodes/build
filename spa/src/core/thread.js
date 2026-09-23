import { agentName } from "./agentName.js";
import { isAgentMessage } from "./unreadAnchor.js";
import { esc } from "./text.js";
import { renderMarkdown } from "./markdown.js";
import { conversationRoute, hashFromRoute } from "./router.js";
import { RENDERED_FOLD_ATTRIBUTE, patchElement, patchInnerHtml } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { followConversation, paintKeepingPlace } from "./paintKeepingPlace.js";
import { paintRunsShowingLatest } from "./activityRunScroll.js";
import { EVENT_META, eventLabel, isStartupEvent } from "./threadEvents.js";
import { activityRunSummary, digestCovering, firstLine, mergeActivityDigests } from "./activityDigest.js";
import {
  autoGrow,
  composerHtml,
  composerPartIds,
  attachmentGlyphHtml,
  formatAttachmentSize,
  isImageAttachment,
  mountComposerAttachments,
  sendControlHtml,
} from "./composer.js";
import { mountSplitMenu } from "./splitButton.js";
import { outcomeMarkHtml } from "./outcomeMark.js";
import { providerLabel } from "./modelPicker.js";
import { viewingContextChipsHtml } from "./viewingContext.js";
import { ICON_CHECK } from "./icons.js";
import { openThreadAttachmentLightbox } from "./threadAttachmentLightbox.js";
import { setMotionRowHtml } from "./motion.js";
import { issueCardHtml } from "./trackerMessageCard.js";
import { issueActionLineHtml } from "./trackerActionLine.js";
import { isIssueNotice, issueNoticeLineHtml, issueNoticeOf } from "./trackerNotice.js";
import { isTransientTransportError } from "./transientRead.js";
import { recordConnectionDiagnostic } from "./connectionDiagnostics.js";
import { buildNoticeSummary, noticeHasMore } from "./buildNoticeLine.js";

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

// A conversation is held as a WINDOW over it: the newest items, widened
// upwards as the reader scrolls back. The record on disk is that window
// (core/conversationCache.js) and the sync layer is what fills it; what the
// surfaces still name here is how much of one an answer is asked to carry.
//
// How much conversation a first load asks for. The daemon clamps whatever it
// hears, so this is a request rather than a promise — but it has to be made:
// a read that names no bound gets the conversation whole, which is the only
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

// ─── Provisional items ───────────────────────────────────────────────────────
//
// A message sent from this tab is on the conversation the moment it is written,
// and the wire says so a round trip later. In between it stands in the thread
// record itself — the same record every other item is in, because a view that
// kept a second store would have to merge the two on every paint and would
// still show the two out of order.
//
// It is held under the OPERATION carrying it rather than under a sequence,
// since it has none: the conversation's counter belongs to the bridge. When the
// post is acknowledged the sequence it was written at is stamped on, which is
// what puts the message in its place in the order — but the key does not move,
// because the stand-in is still a stand-in until the item itself arrives.

const PROVISIONAL_KEY_PREFIX = "provisional:";

/** The key the record holds a sent-but-unechoed message under. */
export const provisionalItemKey = (operationId) => `${PROVISIONAL_KEY_PREFIX}${operationId}`;

/** Whether an item is this tab's own stand-in rather than the conversation's.
 *  An item off the wire names the operation that made it too; what makes this
 *  one provisional is that nothing has confirmed it yet. */
export const isProvisionalItem = (item) => item?.data?.provisional === true && !!item?.data?.operation_id;

/** The key an item is held under: its sequence, or the operation standing in
 *  for one. */
export const threadItemKey = (item) =>
  isProvisionalItem(item) ? provisionalItemKey(item.data.operation_id) : String(item?.data?.sequence ?? "");

/** The message the panel draws while the post carrying it is in flight. */
export const provisionalThreadItem = ({ operationId, message = {}, sequence = null, deliveryStatus = "queued" }) => ({
  type: "message",
  data: {
    role: "user",
    provisional: true,
    operation_id: operationId,
    sequence,
    body: message.body || "",
    attachments: message.attachments || [],
    ...(message.viewing_context ? { viewing_context: message.viewing_context } : {}),
    created_at: new Date().toISOString(),
    delivery_status: deliveryStatus,
  },
});

/** Where an item sits in the record. A provisional item with no sequence yet
 *  is newer than everything the bridge has counted, which is where the reader
 *  just put it. */
const orderingSequence = (item) => {
  const held = item?.data?.sequence;
  const sequence = held == null ? NaN : Number(held);
  return Number.isFinite(sequence) ? sequence : Number.MAX_SAFE_INTEGER;
};

/** Whether an arriving item is the real one behind a stand-in.
 *
 *  The operation it names, first and normally: a message posted under one
 *  wears it on the item, on a page and on a push alike (bridge
 *  `ThreadMessage.operation_id`), which is the only thing that can say "these
 *  are your own words coming back" — the sequence belongs to the bridge and
 *  the browser had none to wait under.
 *
 *  The sequence the post was acknowledged at, second: a daemon that took the
 *  post without binding it to an operation says nothing on the item, and the
 *  receipt is then all there is to match on. */
const standsInFor = (provisional, arrived) => {
  const operationId = arrived?.data?.operation_id;
  if (operationId && operationId === provisional.data.operation_id) return true;
  const sequence = provisional.data.sequence;
  return sequence != null && arrived?.data?.sequence === sequence;
};

/**
 * The record's items after an arrival.
 *
 * Held by key, so a re-shipped item replaces the copy in hand rather than
 * doubling it, and a stand-in leaves as its own message arrives. Sorted by
 * sequence, because neither a push nor a page promises an order and the
 * reader's own message has no sequence to arrive in.
 */
export function mergeThreadItems(held = [], arriving = []) {
  const byKey = new Map(held.map((item) => [threadItemKey(item), item]));
  for (const arrived of arriving) {
    for (const [key, item] of [...byKey]) {
      if (isProvisionalItem(item) && standsInFor(item, arrived)) byKey.delete(key);
    }
    byKey.set(threadItemKey(arrived), arrived);
  }
  return [...byKey.values()].sort((one, other) => orderingSequence(one) - orderingSequence(other));
}

/** The items without one operation's stand-in — a send the bridge refused. The
 *  list itself where it holds none, so a caller can tell nothing happened. */
export function withoutProvisionalItem(items = [], operationId) {
  const key = provisionalItemKey(operationId);
  const kept = items.filter((item) => threadItemKey(item) !== key);
  return kept.length === items.length ? items : kept;
}

/** The items with the sequence a post was acknowledged at stamped onto the
 *  stand-in waiting for it, and re-seated where that sequence puts it. The list
 *  itself where it holds no such stand-in, so a caller can tell nothing
 *  happened.
 *
 *  Re-seated rather than left in place, because until the receipt lands the
 *  stand-in is ordered LAST: an agent's reply arriving in between takes a
 *  higher sequence than the message it is a reply to, and leaving the stand-in
 *  where it stood would draw the reader's own words under the answer to them. */
export function acknowledgeProvisionalItem(items = [], operationId, sequence, deliveryStatus = "sent") {
  const key = provisionalItemKey(operationId);
  if (!items.some((item) => threadItemKey(item) === key)) return items;
  return items
    .map((item) =>
      threadItemKey(item) === key
        ? { ...item, data: { ...item.data, sequence, delivery_status: deliveryStatus } }
        : item,
    )
    .sort((one, other) => orderingSequence(one) - orderingSequence(other));
}

/**
 * The panel's view of one conversation's record.
 *
 * It holds no conversation of its own any more: the record is the conversation
 * (core/conversationCache.js), and this is the shape the timeline is drawn
 * from, kept beside the controller that owns the panel so a repaint does not
 * have to go back to disk. `seedWindow` opens a record into it, and every
 * write to that record opens it again.
 */
export function createThreadCache() {
  let accumulatedItems = [];
  // What each activity run that touches the window totals, keyed by the run's
  // first sequence. A page ships a bounded slice of a run and says here what
  // the whole of it came to.
  let activityDigests = [];
  // Whether there is conversation above the window. Only a paged answer knows,
  // and the record remembers what the last one said.
  let olderItemsRemain = false;
  // The newest counter value the record has taken delivery of — the cursor the
  // sync layer reads forward from, and what a repaint is measured against.
  let deliveredSequence = 0;
  // How long the conversation is, where the wire has said.
  let knownTotalItems = null;

  const highestCursorSequence = (items) =>
    items.reduce(
      (highest, item) => Math.max(highest, item.data?.sequence || 0, item.data?.updated_sequence || 0),
      0,
    );

  const forgetTheWindow = () => {
    accumulatedItems = [];
    activityDigests = [];
    olderItemsRemain = false;
    deliveredSequence = 0;
    knownTotalItems = null;
  };

  return {
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
    reset() {
      forgetTheWindow();
    },
    // The window as it stands.
    readWindow() {
      if (!accumulatedItems.length) return null;
      return { items: accumulatedItems, olderItemsRemain, deliveredSequence, knownTotalItems, activityDigests };
    },
    // Open a record's window. The record is the conversation, so this replaces
    // whatever was held rather than merging into it — including the reader's
    // own widening, which is written back to the record before it gets here.
    // Answers whether there is a conversation to draw.
    seedWindow(saved) {
      if (!saved || !Array.isArray(saved.items) || !saved.items.length) {
        forgetTheWindow();
        return false;
      }
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

/// Everything a reference carries besides its kind, and the dataset key each
/// rides on. One table, read forwards by the render and backwards by the
/// wiring, so a field written onto a chip cannot be forgotten on the way back
/// off it — which is how a reference lost the line it pointed at.
const LINK_FIELDS = Object.freeze([
  { field: "path", data: "path" },
  { field: "issue_id", data: "issueId" },
  { field: "plan_id", data: "planId" },
  { field: "stage_id", data: "stageId" },
  { field: "implementation_id", data: "implementationId" },
  { field: "run_id", data: "runId" },
  { field: "worktree_id", data: "worktreeId" },
  { field: "sha", data: "sha" },
]);

/// The attribute a dataset key is written as: `issueId` rides on
/// `data-issue-id`, which is the one rule the DOM already has for the pair.
const datasetAttribute = (data) => `data-${data.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`;

/// What a reference is worth writing onto its chip. An empty string and a
/// missing field say the same nothing.
const linkFieldWritten = (value) => value != null && value !== "";

function linkAttributes(link) {
  return [
    `data-kind="${esc(link.kind || "")}"`,
    ...LINK_FIELDS.filter(({ field }) => linkFieldWritten(link[field])).map(
      ({ field, data }) => `${datasetAttribute(data)}="${esc(link[field])}"`,
    ),
  ].join(" ");
}

/// What a chip reads back off itself when it is pressed — the reference the
/// render was given, as far as the table carries it.
const linkFromDataset = (dataset) => LINK_FIELDS.reduce(
  (link, { field, data }) => (dataset[data] ? { ...link, [field]: dataset[data] } : link),
  { kind: dataset.kind },
);

/// The fields a reference is named by, best first.
const LABEL_FIELDS = ["path", "implementation_id", "run_id", "worktree_id", "sha"];

/// Kinds a stored conversation may still carry but nothing writes any more:
/// agents no longer attach file links, and the recovery agent is gone. They
/// draw no chip — there is nowhere left for one to go.
const RETIRED_LINK_KINDS = new Set(["file", "recovery"]);

const linkLocation = (link) => LABEL_FIELDS.map((field) => link[field]).find(Boolean) || "Open";

/// One reference, as the chip under a message: a button that opens the work
/// item it names.
function linkChipHtml(link) {
  return `<button type="button" class="thread-reference" ${linkAttributes(link)}>${esc(linkLocation(link))}</button>`;
}

function linksHtml(links) {
  const shown = (links || []).filter((link) => link && !RETIRED_LINK_KINDS.has(link.kind));
  if (!shown.length) return "";
  return `<div class="thread-references">${shown.map(linkChipHtml).join("")}</div>`;
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
const ATTACHMENT_CACHE_MAX = 40;

/** UI state owned by one canonical conversation inside one application/device
 * repository. Keeping these maps behind an instance prevents equal attachment
 * paths and offer ids in unrelated scopes from becoming the same browser state. */
export function createThreadState({ ownerId = "" } = {}) {
  const attachmentDataUrls = new Map();
  const pendingAttachmentLoads = new Map();
  /// Paths whose fetch died on the wire rather than being refused (#30).
  ///
  /// Three states, not two: held bytes, a refusal that is final, and this — a
  /// request that was never answered because nothing carried it. Remembering
  /// this one as a refusal is how Zech's 789 KB screenshot read "unavailable"
  /// for the rest of the tab's life over a path that came back twenty seconds
  /// later, with a hard refresh as the only cure. Remembering nothing at all is
  /// not the answer either: the timeline repaints every second and a half, and a
  /// dead window is a minute of re-asking a wire that is not there.
  ///
  /// So it is held, the figure keeps saying "loading", and the set is emptied
  /// when the device reconnects.
  const deferredAttachments = new Set();
  const pendingChoices = new Map();
  const sendingChoices = new Set();
  const openSentMessages = new Set();
  const openArrivals = new Set();
  let live = true;

  return Object.freeze({
    ownerId,
    active: () => live,
    attachment: (path) => attachmentDataUrls.get(path),
    rememberAttachment(path, dataUrl) {
      if (!live) return;
      deferredAttachments.delete(path);
      if (attachmentDataUrls.size >= ATTACHMENT_CACHE_MAX) {
        attachmentDataUrls.delete(attachmentDataUrls.keys().next().value);
      }
      attachmentDataUrls.set(path, dataUrl);
    },

    /// This fetch was never answered because nothing was carrying it. Held
    /// rather than remembered as a refusal, and asked again after the reconnect.
    deferAttachment(path) {
      if (live) deferredAttachments.add(path);
    },

    /// Whether this path is waiting on a wire rather than settled either way.
    /// The renderer reads it to keep drawing "loading", and the wiring reads it
    /// to leave the path alone until there is a session worth asking.
    attachmentDeferred: (path) => deferredAttachments.has(path),

    /// The device is back: everything a dead path cost is worth asking for
    /// again. Only the deferrals go — held bytes are content-addressed and
    /// immutable, and a real refusal is still a refusal.
    retryDeferredAttachments() {
      const waiting = [...deferredAttachments];
      deferredAttachments.clear();
      return waiting;
    },
    loadAttachment(path, load) {
      if (attachmentDataUrls.has(path)) return Promise.resolve(attachmentDataUrls.get(path));
      const held = pendingAttachmentLoads.get(path);
      if (held) return held;
      let pending;
      try {
        pending = Promise.resolve(load());
      } catch (error) {
        pending = Promise.reject(error);
      }
      pendingAttachmentLoads.set(path, pending);
      const settled = () => {
        if (pendingAttachmentLoads.get(path) === pending) pendingAttachmentLoads.delete(path);
      };
      pending.then(settled, settled);
      return pending;
    },
    sentIsOpen: (key) => openSentMessages.has(key),
    openSentMessage(key, open) {
      if (!live) return;
      if (open) openSentMessages.add(key);
      else openSentMessages.delete(key);
    },
    arrivalIsOpen: (key) => openArrivals.has(key),
    openArrival(key, open) {
      if (!live) return;
      if (open) openArrivals.add(key);
      else openArrivals.delete(key);
    },
    choice: (key) => pendingChoices.get(key) || new Set(),
    choose(key, optionIds) {
      if (live) pendingChoices.set(key, new Set(optionIds));
    },
    isSending: (key) => sendingChoices.has(key),
    beginSending(key) {
      if (live) sendingChoices.add(key);
    },
    finishSending(key, accepted) {
      if (!live) return false;
      sendingChoices.delete(key);
      if (accepted) pendingChoices.delete(key);
      return true;
    },
    snapshot() {
      const picks = [...pendingChoices].map(([key, chosen]) => `${key}=${[...chosen].sort().join(",")}`);
      return { choiceState: picks.sort().join("|"), sending: [...sendingChoices].sort().join("|") };
    },
    dispose() {
      live = false;
      attachmentDataUrls.clear();
      pendingAttachmentLoads.clear();
      deferredAttachments.clear();
      pendingChoices.clear();
      sendingChoices.clear();
      openSentMessages.clear();
    },
  });
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
function attachmentsHtml(attachments, threadState) {
  if (!attachments || !attachments.length) return "";
  return `<div class="thread-attachments">${attachments
    .map((attachment) => {
      const path = esc(attachment.path || "");
      const name = esc(attachment.name || attachment.path || "file");
      const size = esc(formatAttachmentSize(attachment.size));
      if (isImageAttachment(attachment.mime)) {
        // Three states, and the caption says which: nothing (it is coming or it
        // is here), refused (it is not coming), and waiting on a wire that is
        // not there — which is a picture still loading, not a picture gone.
        const refused = threadState.attachment(attachment.path) === null ? " unavailable" : "";
        const waiting = !refused && threadState.attachmentDeferred?.(attachment.path) ? " waiting" : "";
        return `<figure class="thread-attachment-figure${refused}${waiting}">
          <button type="button" class="thread-attachment-preview" aria-label="Open ${name}">
            <img class="thread-attachment-image" data-attachment-path="${path}" alt="${name}">
          </button>
          <figcaption><span class="thread-attachment-name">${name}</span> <span class="thread-attachment-size">${size}</span></figcaption>
        </figure>`;
      }
      return `<button type="button" class="thread-attachment" data-attachment-path="${path}" data-attachment-name="${name}" title="Download ${name}">
        ${attachmentGlyphHtml(attachment.name, attachment.mime, "thread-attachment-glyph")}
        <span class="thread-attachment-meta">
          <span class="thread-attachment-name">${name}</span>
          <span class="thread-attachment-size">${size}</span>
        </span>
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
/// What a pick is filed under. Every conversation numbers its messages from
/// one, so the message id alone would put one thread's picks on another's
/// chips the moment the reader switched agents.
const offerKey = (threadId, messageId) => `${threadId || ""}::${messageId || ""}`;

/// What is marked on an offer's chips: what was actually sent, once there is
/// such a thing; what is picked and not yet sent, while it can still be sent;
/// and nothing at all on an offer that went by unanswered — picking is not
/// choosing, so an offer overtaken mid-pick leaves no mark.
const chosenOn = (message, live, key, threadState) => {
  if ((message.selected_options || []).length) return new Set(message.selected_options);
  return live ? threadState.choice(key) : new Set();
};

/// The actions the agent suggested taking in answer to its message.
///
/// `live` is the whole of whether they can be pressed: an offer is answerable
/// only while it is the last thing said and nothing has been chosen on it, so
/// anything said afterwards — by either side — leaves it dim exactly as it
/// stands. A choice already made keeps its chips marked, because that mark is
/// the conversation's only record of what the reader pressed.
function optionsHtml(message, live, key, threadState) {
  const options = message.options || [];
  if (!options.length) return "";
  const answered = (message.selected_options || []).length > 0;
  const shut = answered || !live;
  const chosen = chosenOn(message, live, key, threadState);
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

const messageContextHtml = (message) => message.viewing_context?.items?.length
  ? `<div class="message-viewing-context">${viewingContextChipsHtml(message.viewing_context)}</div>` : "";

const DELIVERY_STATUS_META = {
  queued: { label: "Queued", description: "Queued for the agent" },
  submitted: { label: "Queued", description: "Queued for the agent" },
  sent: { label: "Sent", description: "Sent to the agent" },
  seen: { label: "Seen", description: "Seen by the agent" },
  uncertain: { label: "Delivery uncertain", description: "Message delivery is uncertain" },
  failed: { label: "Failed", description: "Message delivery failed" },
};

function deliveryStatusHtml(message) {
  const token = message.delivery_status;
  const meta = DELIVERY_STATUS_META[token];
  if (meta) {
    return `<span class="thread-status delivery-status ${esc(token)}" data-delivery-status="${esc(token)}" role="status" aria-label="${esc(meta.description)}">${esc(meta.label)}</span>`;
  }
  // Threads written before native delivery receipts retain their historical
  // read marker. New bridges provide delivery_status, including `sent` when
  // the provider cannot report that an agent has seen a message.
  return `<span class="thread-status" role="img" aria-label="${message.seen_at ? "Read" : "Sent"}">${ICON_CHECK}${message.seen_at ? ICON_CHECK : ""}</span>`;
}


function messageFooterHtml(message) {
  // The delivery report is about a send the reader made: a message that arrived
  // from another agent is on the user's role and was nobody's send but the
  // sender's, so it wears the time and nothing else.
  const said = message.role === "user" && !message.from_agent;
  const status = said
    ? deliveryStatusHtml(message)
    : "";
  const time = timeHtml(message.created_at);
  return status || time ? `<div class="thread-message-footer">${status}${time}</div>` : "";
}

const resolvedRevisionHtml = (message) => message.resolved_by_revision
  ? `<div class="thread-message-resolution"><button class="thread-revision-link" data-revision="${esc(message.resolved_by_revision)}">Resolved in ${esc(message.resolved_by_revision)}</button></div>`
  : "";

/// How much of a sender's id a chip can hold. Enough to tell two agents
/// apart at a glance; the whole id is on the label a reader can reach for.
const AGENT_CHIP_CHARS = 4;

/// The mark an attributed message wears: the sender's id, short enough to sit
/// in the avatar's circle, and "Agent" when the id has nothing to show.
function agentChipLabel(id) {
  const trimmed = String(id || "").trim();
  const body = trimmed.includes("-") ? trimmed.slice(trimmed.indexOf("-") + 1) : trimmed;
  const short = body.replace(/[^a-z0-9]/gi, "").slice(0, AGENT_CHIP_CHARS);
  return short ? short.toUpperCase() : "Agent";
}

/// Who a bubble belongs to: the human's initial, or the agent's. A message
/// that arrived from another agent wears neither — it is drawn as a bubble of
/// its own, under the conversation it was said in.
function avatarHtml(user) {
  return `<span class="thread-avatar" aria-hidden="true">${user ? "Y" : "A"}</span>`;
}

/// What a conversation nobody has named is called, so the second half of a
/// reference is always something a reader can aim at.
const UNTITLED_CONVERSATION = "Untitled conversation";

/// What the second link says: the sender's NAME when it has one, else the topic
/// its conversation carries. A name tells the reader which agent wrote; a topic
/// tells them what that agent is on, and the name is the better answer to "who
/// is this".
const topicLabel = (reference) =>
  agentName(reference) || String(reference.topic || "").trim() || UNTITLED_CONVERSATION;

/// Where the other end of an agent-to-agent message lives: the page its owner
/// is, and the conversation itself.
///
/// The owner names half of the route and the reader's own page names the rest —
/// a workspace belongs to the project whose page the rail is standing on, and
/// every route is written against the machine holding the conversation. A
/// record written before senders carried an owner names no page at all, and
/// none is invented for it.
function conversationLinks(reference, place) {
  const owner = reference.owner;
  if (!owner || !owner.id) return null;
  const workspace = owner.kind === "workspace";
  const projectId = workspace ? place.projectId : owner.id;
  if (!projectId) return null;
  const page = {
    kind: owner.kind,
    projectId,
    deviceId: place.deviceId ?? null,
    workspaceId: workspace ? owner.id : null,
  };
  return {
    name: owner.name || owner.id,
    ownerHref: hashFromRoute(conversationRoute(page)),
    topicHref: hashFromRoute(conversationRoute({ ...page, agentId: reference.id })),
  };
}

/// The sender of a message from before owners rode the wire: four characters of
/// the id, with the whole of it said aloud. It is all such a record holds —
/// there is no page to point at — so it points nowhere.
const senderChipHtml = (id, prefix) =>
  `<span class="${prefix}-chip" role="img" aria-label="Sent by agent ${esc(id)}" title="Sent by agent ${esc(id)}">${esc(agentChipLabel(id))}</span>`;

/// A conversation elsewhere, named as two links: the workspace or project it
/// belongs to, and the conversation on it. One writer, so a message that
/// arrived and a message that was sent say where they point the same way.
function conversationLinksHtml(reference, place, prefix) {
  const links = conversationLinks(reference, place);
  if (!links) return senderChipHtml(reference.id, prefix);
  return `<a class="${prefix}-owner" href="${esc(links.ownerHref)}">${esc(links.name)}</a><span class="${prefix}-sep" aria-hidden="true"> › </span><a class="${prefix}-topic" href="${esc(links.topicHref)}">${esc(topicLabel(reference))}</a>`;
}

/// Everything inside a message's card. Shared by the reader's own bubble and by
/// the bubble another agent's words arrive in: the two differ in where they sit
/// and what colour they are, and in nothing a message holds.
/// The issue a message handed over, drawn as a card above its body.
///
/// Assignment is dispatch: the issue arrives as an ordinary message carrying a
/// `from_issue` envelope, and the body is that issue rendered as prose so a
/// harness that never learns about the envelope still receives the whole of it.
/// Every message carrying one gets a card, whoever sent it — an assignment the
/// user made carries no `from_agent` and one an agent made does — and every
/// message carrying none gets nothing at all, which is every other message.
///
/// Its fold is the arrival fold: the same length, the same measurement, the
/// same press, and the same memory on the thread state. The key is the
/// message's own with a suffix, so an arrival that hands over an issue can
/// fold its report and its issue independently.
function handedIssueHtml(message, context) {
  const envelope = message.from_issue;
  if (!envelope) return "";
  const key = `${messageKey(message)}-issue`;
  const bodyId = `thread-issue-${esc(key)}`;
  const long = bodyRunsLong(envelope.body);
  const open = !long || context.threadState.arrivalIsOpen(key);
  return issueCardHtml(envelope, {
    place: context.place,
    bodyId,
    folded: !open,
    pressHtml: long ? arrivalPressHtml(key, bodyId, open) : "",
  });
}

/// What a message says beyond the issue it hands over. The body of a hand-off
/// is the issue rendered as prose — `#12 Title`, the issue's body — followed
/// by whatever the sender added, so a harness that never learns about the
/// envelope still receives the whole issue. The card above already draws the
/// issue, so drawing the body whole would say the issue twice; only the part
/// after the prose is the sender's own words. A body that does not start
/// with the prose (an older bridge, a different shape) is drawn whole.
function bodyBeyondIssue(message) {
  const envelope = message.from_issue;
  const body = String(message.body || "");
  if (!envelope) return body;
  const issueBody = String(envelope.body || "").trim();
  const prose = `#${envelope.number ?? ""} ${envelope.title || ""}${issueBody ? `\n\n${issueBody}` : ""}`;
  return body.startsWith(prose) ? body.slice(prose.length).trim() : body;
}

function messageCardHtml(message, agentLabel, context) {
  const { live, offer, threadState } = context;
  const body = bodyBeyondIssue(message);
  // `done` is message metadata, not a presentation type: on a thread written
  // before outcomes were message statuses it flags the send that followed the
  // timeline's done event, and such a message renders like every other one.
  // What marks a message is `outcome` — the whole record of a reported outcome.
  // The body IS the report: the agent's `done` summary, in markdown, which is
  // why no card of lists sits under it any more.
  // renderMarkdown escapes all input before adding its fixed safe tag set.
  return `
      ${outcomeMarkerHtml(message.outcome, agentLabel)}
      ${resolvedRevisionHtml(message)}
      ${anchorLabel(message.anchor)}
      ${messageContextHtml(message)}
      ${handedIssueHtml(message, context)}
      ${body ? `<div class="thread-body markdown">${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ renderMarkdown(body, { links: context.refLinks })}</div>` : ""}
      ${attachmentsHtml(message.attachments, threadState)}
      ${linksHtml(message.links)}
      ${optionsHtml(message, live, offer, threadState)}
      ${messageFooterHtml(message)}
    `;
}

/// How far an arrival is let down the page before it is folded, and roughly
/// how many characters one of those lines holds in the panel.
///
/// The width is an estimate on purpose: what is being answered is "does this
/// bury the conversation", and that question does not get a better answer from
/// measuring the glyphs. The fold itself is the stylesheet's, over the markdown
/// as rendered, so a message that estimates just over the line is folded at its
/// true fifth line and not at a guess.
const ARRIVAL_LINES = 5;
const ARRIVAL_LINE_CHARS = 72;

/// Roughly how many lines of the panel a body will take: every line of the
/// source, plus what each of them wraps into. A blank line counts nothing — it
/// is the space between paragraphs, not a line of words.
const arrivalLineCount = (body) =>
  String(body || "")
    .split("\n")
    .reduce((lines, line) => lines + Math.ceil(line.trim().length / ARRIVAL_LINE_CHARS), 0);

/// Whether a body is long enough to be worth folding. A body that already fits
/// is left whole, and is given no press: there would be nothing behind it.
///
/// A handed-over issue's card folds by this same rule and at this same length
/// — it is handed the answer rather than asking again, so there is one reading
/// of "does this bury the conversation" and no second one to drift from it.
const bodyRunsLong = (body) => arrivalLineCount(body) > ARRIVAL_LINES;

const arrivalRunsLong = (message) => bodyRunsLong(message.body);

/// The press under a folded arrival, which is the whole of the affordance: the
/// two words are swapped by the stylesheet off `aria-expanded`, so the button
/// says one thing at a time and the state is read from one place.
const arrivalPressHtml = (key, cardId, open) =>
  `<button type="button" class="thread-arrival-press" data-arrival-message="${esc(key)}" aria-controls="${cardId}" aria-expanded="${open ? "true" : "false"}"><span class="thread-arrival-more">Show more</span><span class="thread-arrival-less">Show less</span></button>`;

/// A message another agent sent into this conversation.
///
/// It is somebody else speaking, so it takes the side of the thread everything
/// said to this agent takes — the left — and a colour of its own: the reader's
/// own bubble is the reader's voice, and these are not their words. Above it
/// stands the only thing that makes it readable at all, which is where it came
/// from: the workspace or project, and the conversation it was said in.
///
/// What arrives is usually a report, and a report is long: laid into the
/// conversation whole it buries everything said around it. So a long one is
/// folded to its first lines with a press underneath, and what the reader has
/// opened is remembered outside the markup — the same way a sent message
/// remembers it — so a repaint does not shut it under them.
function arrivedMessageHtml(message, agentLabel, context) {
  const key = messageKey(message);
  const cardId = `thread-arrival-${esc(key)}`;
  const long = arrivalRunsLong(message);
  const open = !long || context.threadState.arrivalIsOpen(key);
  return `<article class="thread-message thread-comment from-agent"${sequenceAttribute(message)}>
    <div class="thread-from">${conversationLinksHtml(message.from_agent, context.place, "thread-from")}</div>
    <div class="thread-comment-card${open ? "" : " thread-arrival-folded"}" id="${cardId}">${messageCardHtml(message, agentLabel, context)}</div>
    ${long ? arrivalPressHtml(key, cardId, open) : ""}
  </article>`;
}

/// How a message is remembered while the reader has it open — a sent one, an
/// arrival, anything else that folds. The id, which is stable across every
/// repaint; the sequence for a conversation rendered without one.
const messageKey = (message) => String(message.id || message.sequence || "");

/// The second line of a sent message, which is the press that opens it: the
/// first line of what was sent, or — once it is open — the way to shut it
/// again.
const sentPressHtml = (key, bodyId, open, preview) =>
  `<button type="button" class="thread-sent-preview" data-sent-message="${esc(key)}" aria-controls="${bodyId}" aria-expanded="${open ? "true" : "false"}"><span class="thread-sent-first">${esc(preview)}</span><span class="thread-sent-shut">Hide</span></button>`;

/// A message this agent sent to another agent.
///
/// It counts as a message — it breaks the activity around it in two, the way
/// anything said does — but almost none of it is for this reader, who wrote
/// neither the words nor the send. So it is two lines: where it went, and how
/// it opened. The whole of it is one press away.
///
/// Open, the second line is the press that shuts it again rather than the first
/// line a second time: the body underneath already starts with those words.
function sentMessageHtml(message, { place, threadState, refLinks }) {
  const key = messageKey(message);
  const bodyId = `thread-sent-${esc(key)}`;
  const open = threadState.sentIsOpen(key);
  const body = String(message.body || "");
  // renderMarkdown escapes all input before adding its fixed safe tag set.
  return `<article class="thread-message thread-sent"${sequenceAttribute(message)}>
    <div class="thread-sent-head">
      <span class="thread-sent-label">Sent a message to ${conversationLinksHtml(message.sent_to, place, "thread-sent")}</span>
      ${timeHtml(message.created_at)}
    </div>
    ${sentPressHtml(key, bodyId, open, firstLine(body).trim())}
    <div class="thread-body markdown" id="${bodyId}"${open ? "" : " hidden"}>${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ renderMarkdown(body, { links: refLinks })}</div>
  </article>`;
}

/// Build's own words in an agent's conversation, as ONE line.
///
/// Zech: "notifications are a single line left aligned". They were a bubble —
/// the reader's bubble, on the reader's side, in the reader's colour — and a
/// restart notice that says "assume nothing you were doing finished" reads
/// very differently when it looks like the reader typed it.
///
/// Every one of these is written for the AGENT: the restart notice tells it
/// what to distrust, the reminder lists what it still holds. All of that has
/// to reach the agent and none of it has to be on screen, so the body is kept
/// whole behind a press and the line is a summary of it
/// (core/buildNoticeLine.js).
///
/// A notice about an issue is the same row with the same look, and its line is
/// the issue's (core/trackerNotice.js): the whole of it opens the issue, which
/// is a better press than revealing prose about it.
function noticeMessageHtml(message, context) {
  const row = (inner) =>
    `<article class="thread-message thread-issue-line thread-notice thread-quiet-row"${sequenceAttribute(message)}>${inner}</article>`;

  if (isIssueNotice(message)) {
    return row(issueNoticeLineHtml(issueNoticeOf(message), {
      place: context.place,
      agentLabels: context.agentLabels,
      projectName: context.place?.projectName || "",
    }));
  }

  const summary = buildNoticeSummary(message);
  // `<details>` and not a button: the browser owns the toggle, which is the
  // keyboard and the screen reader handled without this file re-implementing
  // either. A notice with nothing more to say is a line and no press at all.
  if (!noticeHasMore(message, summary)) {
    return row(`<span class="thread-issue-notice"><span class="thread-issue-said">${esc(summary)}</span></span>`);
  }
  return row(`<details class="thread-notice-more">
      <summary class="thread-issue-notice"><span class="thread-issue-said">${esc(summary)}</span></summary>
      <div class="thread-notice-body markdown">${/* nosemgrep: javascript.express.security.injection.raw-html-format.raw-html-format */ renderMarkdown(message.body || "", { links: context.refLinks })}</div>
    </details>`);
}

/// One message, drawn as whichever of the three things it is: what this agent
/// sent elsewhere, what another agent sent here, or the plain bubble everything
/// else has always been.
///
/// The sequence rides the row: it is how the timeline says which message a row
/// stands for, and how the panel reports what the reader's viewport has reached
/// (`readThroughSequence`).
/// An agent acting on an issue, as one line in its own voice.
///
/// Its own row rather than a card inside the ordinary bubble: an agent that
/// files, assigns, moves and comments across a session would otherwise bury
/// what it SAID under its own bookkeeping. The sequence rides it like any
/// other message, so it reads in order and counts as unread.
function issueActionMessageHtml(message, context) {
  return `<article class="thread-message thread-issue-line thread-action thread-quiet-row"${sequenceAttribute(message)}>
    ${issueActionLineHtml(message.issue_action, {
      place: context.place,
      // The same labels the notice line reads, so an agent named once is
      // named the same way in both.
      agentLabels: context.agentLabels,
      projectName: context.place?.projectName || "",
    })}
  </article>`;
}


/// The three kinds of message an agent RECEIVES, and the one it sends.
///
/// Zech: "Messages to agents and messages from the user are different things
/// and should never look the same. Agents receive 3 kinds of messages: from
/// the user, from other agents, and notifications."
///
/// Named here, once, because the looks they get are mutually exclusive and
/// the readings used to be a ladder of separate conditions that could each be
/// true. That is how Build's restart notice ended up wearing the reader's own
/// bubble on the reader's own side: `from_build` was checked after a special
/// case for one KIND of notice, so the other kind fell through to the bubble
/// every instruction wears.
export const INCOMING_KINDS = Object.freeze({
  /// The reader typed it. Right, and the reader's colour.
  user: "user",
  /// Another agent sent it here — the project's agent, a workspace agent, a
  /// hand-off. Left, neutral, folded, with the sender named.
  agent: "agent",
  /// Build wrote it about the work, not to the reader. One quiet line.
  notice: "notice",
});

/**
 * Which of the three a message is — for a message that ARRIVED.
 *
 * Order is the whole of it: Build's mark outranks everything, because a
 * notice carries the reader's own role and would otherwise read as something
 * the reader wrote. A sender outranks the role for the same reason.
 */
export function incomingKindOf(message) {
  if (message?.from_build) return INCOMING_KINDS.notice;
  if (message?.from_agent) return INCOMING_KINDS.agent;
  return INCOMING_KINDS.user;
}

const isReaderMessage = (message) => message?.role === "user" && !message.issue_action
  && !message.sent_to && incomingKindOf(message) === INCOMING_KINDS.user;

function messageHtml(message, agentLabel, context) {
  // This agent's own doings first: an action line is it saying what it just
  // did here, and a sent message is it speaking elsewhere. Neither arrived.
  if (message.issue_action) return issueActionMessageHtml(message, context);
  if (message.sent_to) return sentMessageHtml(message, context);
  const kind = incomingKindOf(message);
  if (kind === INCOMING_KINDS.notice) return noticeMessageHtml(message, context);
  if (kind === INCOMING_KINDS.agent) return arrivedMessageHtml(message, agentLabel, context);
  // An agent's own words are not incoming at all; only the reader's are.
  const user = isReaderMessage(message);
  return `<article class="thread-message thread-comment ${user ? "user" : "agent"}"${sequenceAttribute(message)}>
    ${avatarHtml(user)}
    <div class="thread-comment-card">${messageCardHtml(message, agentLabel, context)}</div>
  </article>`;
}

function userMessageTickHtml({ key, message, index }) {
  const date = new Date(message.created_at || "");
  const when = Number.isNaN(date.getTime()) ? "an earlier time" : date.toLocaleString("en-US", {
    dateStyle: "medium", timeStyle: "short",
  });
  return `<button type="button" class="thread-user-tick" data-key="${esc(key)}" data-user-tick-index="${index}" aria-label="Jump to your message from ${esc(when)}"><span aria-hidden="true"></span></button>`;
}

const USER_TICK_WINDOW = 12;

const userMessageTicks = (built) => built.entries
  .filter(({ item }) => item?.type === "message" && isReaderMessage(item.data))
  .map(({ key, item }, index) => ({ key, message: item.data, index }));

const userMessageNavHtml = (built) => {
  const ticks = userMessageTicks(built);
  // The template is the cache-backed source for windows reached by scrolling.
  // Its contents are inert: only the first twelve buttons are painted.
  return `<nav class="thread-user-nav" aria-label="Your messages"><template class="thread-user-nav-source">${ticks
    .map(userMessageTickHtml).join("")}</template><div class="thread-user-nav-list" data-window-start="0">${ticks
    .slice(0, USER_TICK_WINDOW).map(userMessageTickHtml).join("")}</div></nav>`;
};

function nearestUserMessageIndex(rows, boundary) {
  let previous = -1;
  for (const [index, row] of [...rows].entries()) {
    if (row.getBoundingClientRect().top <= boundary) previous = index;
    else break;
  }
  return previous;
}

function userTickWindowStart(count, active) {
  if (count <= USER_TICK_WINDOW || active < 0) return 0;
  return Math.max(0, Math.min(active - Math.floor((USER_TICK_WINDOW - 1) / 2), count - USER_TICK_WINDOW));
}

function paintUserTickWindow(list, source, start) {
  if (list.dataset.windowStart === String(start) && list.childElementCount === Math.min(USER_TICK_WINDOW, source.childElementCount)) return;
  const visible = [...source.children].slice(start, start + USER_TICK_WINDOW);
  patchList(list, visible, {
    keyOf: (tick) => tick.dataset.key,
    render: (tick) => tick.cloneNode(true),
  });
  list.dataset.windowStart = String(start);
}

function keepUserTickInView(list, tick) {
  if (!list.clientHeight) return;
  if (tick.offsetTop < list.scrollTop) list.scrollTop = tick.offsetTop;
  else if (tick.offsetTop + tick.offsetHeight > list.scrollTop + list.clientHeight) {
    list.scrollTop = tick.offsetTop + tick.offsetHeight - list.clientHeight;
  }
}

function markNearestUserTick(list, previous) {
  for (const tick of list.querySelectorAll(".thread-user-tick")) {
    const active = Number(tick.dataset.userTickIndex) === previous;
    tick.classList.toggle("active", active);
    if (active) tick.setAttribute("aria-current", "location");
    else tick.removeAttribute("aria-current");
    if (active) keepUserTickInView(list, tick);
  }
}

/** The last reader message whose top has reached the viewport, never the next
 * one below it. The group stays pinned while its twelve-tick window follows. */
export function syncUserMessageTicks(scroller) {
  const timeline = scroller?.querySelector(".thread-items");
  const list = scroller?.querySelector(".thread-user-nav-list");
  const source = scroller?.querySelector(".thread-user-nav-source")?.content;
  if (!timeline || !list || !source) return;
  const scrollPadding = Number.parseFloat(getComputedStyle(scroller).scrollPaddingTop) || 0;
  const boundary = scroller.getBoundingClientRect().top + scrollPadding;
  const rows = timeline.querySelectorAll(":scope > .thread-message.user");
  const previous = nearestUserMessageIndex(rows, boundary);
  paintUserTickWindow(list, source, userTickWindowStart(rows.length, previous));
  markNearestUserTick(list, previous);
}

const pendingTickSyncs = new WeakSet();

/** A scroll burst needs one layout read and one tick update per browser frame. */
export function scheduleUserMessageTickSync(scroller) {
  if (!scroller || pendingTickSyncs.has(scroller)) return;
  const browser = scroller.ownerDocument?.defaultView;
  if (!browser?.requestAnimationFrame) {
    syncUserMessageTicks(scroller);
    return;
  }
  pendingTickSyncs.add(scroller);
  browser.requestAnimationFrame(() => {
    pendingTickSyncs.delete(scroller);
    syncUserMessageTicks(scroller);
  });
}

/** Called by the rail's delegated click handler so a repainted tick needs no
 * per-node listener. The button remains keyboard-operable by the browser. */
export function jumpToUserMessage(event) {
  const tick = event.target.closest?.(".thread-user-tick");
  if (!tick) return false;
  const index = Number(tick.dataset.userTickIndex);
  const row = tick.closest(".review-thread")?.querySelectorAll(".thread-items > .thread-message.user")[index];
  if (!row) return false;
  row.scrollIntoView({ behavior: "smooth", block: "start" });
  return true;
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
  if (!body) return `<div class="thread-event thread-activity thread-quiet-row"${sequence}>${head}</div>`;
  return `<details class="thread-event thread-activity thread-quiet-row"${sequence}>
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

/// The newest sequence a run's box stands for — its own rows and the calls they
/// fold — read back off the document, and 0 for a run that is not drawn.
///
/// This is what the client holds about a run, as against what the daemon last
/// said about it in a digest: a delta lands rows on the live tail run without
/// ever refreshing its digest, so the box is the only thing that knows the run
/// has grown.
export function activityRunThroughAt(scroller, runKey) {
  const run = scroller ? scroller.querySelector(`[${ACTIVITY_RUN_ATTRIBUTE}="${runKey}"]`) : null;
  return run ? runSpanAttribute(run, ACTIVITY_RUN_THROUGH_ATTRIBUTE) : 0;
}

/// How much of the conversation the reader has read, as a sequence.
///
/// Reading is per MESSAGE: a row counts once its bottom edge has come into
/// view, which is the moment the reader could have finished it. Everything
/// above the viewport counts too — they scrolled past it.
///
/// Only the timeline's own rows are asked. A run's box stands for every call
/// folded inside it, including the half a page cut away, so reaching the bottom
/// of the box reads the whole span it names; the rows inside it scroll in their
/// own little window and answer for nothing.
///
/// 0 means nothing has been read yet, which is what the daemon's cursor calls
/// never — so a report of 0 is one that moves nothing.
export function readThroughSequence(scroller) {
  const timeline = scroller && scroller.querySelector(".thread-items");
  if (!timeline) return 0;
  const floor = scroller.getBoundingClientRect().bottom;
  return [...timeline.children].reduce((read, row) => {
    const reaches = rowReachesSequence(row);
    if (!reaches || row.getBoundingClientRect().bottom > floor) return read;
    return Math.max(read, reaches);
  }, 0);
}

/// The newest sequence one timeline row stands for: its own for a message or a
/// lifecycle event, the whole folded span for a run, and nothing for a row
/// nobody said (the unread line).
function rowReachesSequence(row) {
  const own = Number(row.getAttribute("data-sequence") ?? NaN);
  if (Number.isFinite(own)) return own;
  return runSpanAttribute(row, ACTIVITY_RUN_THROUGH_ATTRIBUTE) || 0;
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
  return `<details class="thread-activity-group thread-quiet-row" ${RENDERED_FOLD_ATTRIBUTE} ${ACTIVITY_RUN_ATTRIBUTE}="${esc(String(span.key))}" ${ACTIVITY_RUN_FROM_ATTRIBUTE}="${esc(String(span.from))}" ${ACTIVITY_RUN_THROUGH_ATTRIBUTE}="${esc(String(span.through))}"${children === null ? "" : " open"}>
    <summary class="thread-activity-head thread-activity-group-head">
      <span class="thread-event-icon" aria-hidden="true">${esc(summary.icon)}</span>
      <span class="thread-activity-label">Actions</span>
      <span class="thread-activity-count">${summary.count}</span>
      <span class="thread-activity-preview">${esc(summary.meat)}</span>
      ${toolOutcomeHtml(summary.outcome)}
      ${timeHtml(summary.createdAt)}
    </summary>
    ${children === null ? "" : `<div class="thread-activity-group-list">${children}</div>`}
  </details>`;
}

/// The newest sequence a run stands for: its own rows, and the rows they fold.
const runThroughSequence = (run) =>
  run.reduce(
    (newest, row) =>
      (row.activity.rows || []).reduce(
        (highest, folded) => Math.max(highest, folded.sequence || 0),
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
  return [...timelineRowsOf(fetched, view.agentLabel, view.threadId, view), ...live]
    .map((row) => row.html)
    .join("");
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
    entries.push({ key: row.key, html: row.html, item: row.item });
  }
  closeRun();
  return entries;
}

// eslint-disable-next-line complexity -- ratchet: eventHtml is at 11, cap 10 — reduce it, then drop this line
function eventHtml(event, agentLabel = "Agent", foldedChildrenHtml = "") {
  const meta = EVENT_META[event.event] || { label: String(event.event || "event").replaceAll("_", " "), icon: "•" };
  if (meta.activity) return activityHtml(event, meta, agentLabel, foldedChildrenHtml);
  const label = event.event === "compaction" && event.summary
    ? event.summary
    : eventLabel(meta, agentLabel);
  const detail = event.revision_id
    ? `<button class="thread-revision-link" data-revision="${esc(event.revision_id)}">${esc(event.revision_id)}</button>`
    : event.event !== "done" && event.event !== "compaction" && event.summary ? renderMarkdown(event.summary) : "";
  return `<div class="thread-event ${meta.tone || ""}">
    <span class="thread-event-icon" aria-hidden="true">${esc(meta.icon)}</span>
    <div class="thread-event-content"><div><strong>${esc(label)}</strong> ${timeHtml(event.created_at)}</div>${detail ? `<div class="thread-event-detail">${detail}</div>` : ""}${linksHtml(event.links)}</div>
  </div>`;
}

const TOOL_CALL_KIND = "tool_use";

/// How a run of activity folds: which rows hide under which, and what a row
/// stands for.
///
/// A tool call that spawns a subagent owns everything the subagent did — those
/// rows are drawn inside its fold rather than beside it — so the row a reader
/// sees is one row and several. Every reading comes from the same parent map:
/// the html of what folds under a row, the rows that row stands for, and the
/// calls among them.
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
  /// A row and everything folded under it, at every depth, each visited once
  /// however the parent links happen to loop.
  const standingFor = (item, alreadyWalked) => {
    const sequence = sequenceOf(item);
    if (alreadyWalked.has(sequence)) return [];
    const walked = new Set([...alreadyWalked, sequence]);
    const children = childrenByParent.get(sequence) || [];
    return [item, ...children.flatMap((child) => standingFor(child, walked))];
  };
  const callOf = (item) => {
    const event = item.data || {};
    return {
      sequence: sequenceOf(item),
      meat: activityMeat(event, EVENT_META[TOOL_CALL_KIND], agentLabel),
      outcome: event.outcome,
      createdAt: event.created_at,
    };
  };
  const rowsUnder = (item) => standingFor(item, new Set()).map((held) => ({ sequence: sequenceOf(held) }));
  const toolCallsUnder = (item) =>
    standingFor(item, new Set())
      .filter((held) => (held.data || {}).event === TOOL_CALL_KIND)
      .map(callOf);
  return { foldedItems, foldedChildrenHtmlOf, rowsUnder, toolCallsUnder };
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
      // spawned a subagent stands for every row the subagent made, and the
      // calls among them are what the head's line is drawn from.
      rows: folding.rowsUnder(item),
      toolCalls: folding.toolCallsUnder(item),
    },
  };
}

/// A message's row, or no row at all for the two the timeline does not draw.
///
/// `spoken` is whether this is the last thing said, which is the whole of
/// whether its offer can still be answered.
function messageRow(item, index, agentLabel, { threadId, spoken, threadState, place, agentLabels, refLinks }) {
  const message = item.data || {};
  // Old bridges persisted the noisy structured handoff as a chat message.
  if (message.source === "completion" && String(message.body || "").includes("Completion report")) return [];
  // A choice is drawn on the chips that offered it, so the message it sent
  // would be the same words a second time.
  if (message.answers_options_of) return [];
  const offer = offerKey(threadId, message.id);
  const live = spoken && !threadState.isSending(offer);
  const context = { live, offer, threadState, place, agentLabels, refLinks };
  return [{ key: rowKey(message, index), item, html: messageHtml(message, agentLabel, context) }];
}

/// Every top-level row a set of items draws, in order.
///
/// The one reading of what a row is: startup noise is not one, a row folded
/// under the call that spawned it is not one of its own, and two kinds of
/// message are drawn on other rows instead. Used for the conversation itself
/// and for the children of an open run, so a fetched run's rows are the rows
/// the window would have drawn for the same items.
function timelineRowsOf(sourceItems, agentLabel, threadId, { threadState, place, agentLabels, refLinks }) {
  const items = sourceItems.filter((item) => !isStartupEvent(item));
  const folding = threadFolding(items, agentLabel);
  const topLevelItems = items.filter((item) => !folding.foldedItems.has(item));
  // Which message may still be answered with a chip: the last one said, and
  // only that one. An event between it and now changes nothing — a commit
  // landing is not somebody speaking.
  const lastSpoken = topLevelItems.reduce((last, item, index) => (item.type === "message" ? index : last), -1);
  return topLevelItems.flatMap((item, index) =>
    item.type === "message"
      ? messageRow(item, index, agentLabel, { threadId, spoken: index === lastSpoken, threadState, place, agentLabels, refLinks })
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
export function timelineEntries(
  sourceItems,
  agentLabel,
  threadId,
  digests,
  {
    openRuns,
    runItemsOf,
    threadState = createThreadState(),
    unreadFrom,
    place = NOWHERE_IN_PARTICULAR,
    // What this project's agents are called. Only one row reads them — a
    // tracking notice's "X did Y" — and X has to be a name the reader knows.
    agentLabels = {},
    // What a written reference points at (core/referenceTargets.js): the
    // resolver #56 left injectable, so `#42` and `@workspace:build` in a
    // message open the thing they name. None, and they stay prose (#63).
    refLinks = null,
    hiddenByLevel = 0,
  } = {},
) {
  const view = {
    agentLabel,
    agentLabels,
    refLinks,
    threadId,
    threadState,
    place,
    openRuns: openRuns || NO_RUNS_OPEN,
    runItemsOf: runItemsOf || noRunItems,
  };
  const rows = timelineRowsOf(sourceItems, agentLabel, threadId, view);
  const entries = foldActivityRuns(rows, digests, view);
  return {
    entries: withUnreadLine(entries, unreadFrom, sourceItems),
    itemCount: rows.length,
    // How many items the caller's detail level kept OUT of `sourceItems` —
    // which is the whole of whether an empty timeline means nothing was said.
    hiddenByLevel,
  };
}

/** The class the unread line is drawn with, and how the scroll finds it. */
export const UNREAD_LINE_SELECTOR = ".thread-unread-line";

/// The one line in the timeline nobody said: everything below it is what the
/// reader has not read yet.
///
/// Keyed like any other entry, so the reconciler rules it once and takes it
/// away once — a line redrawn on every tick would flicker down a conversation
/// an agent is writing into.
const UNREAD_LINE_ENTRY = {
  key: "unread",
  html: '<div class="thread-unread-line" role="separator"><span>New</span></div>',
};

/// The entries with the unread line ruled into them.
///
/// Only agent message rows can carry the divider. If its original message
/// is outside the window, the first agent reply after it is the visible anchor.
function withUnreadLine(entries, unreadFrom, items) {
  if (!Number.isFinite(unreadFrom)) return entries;
  const agentSequences = new Set(items.filter(isAgentMessage).map((item) => Number(item.data.sequence)));
  const at = entries.findIndex((entry) => Number(entry.key) >= unreadFrom && agentSequences.has(Number(entry.key)));
  if (at < 0) return entries;
  return [...entries.slice(0, at), UNREAD_LINE_ENTRY, ...entries.slice(at)];
}

const NO_RUNS_OPEN = new Set();

/// The page a conversation is drawn on, for a caller that names none: a link
/// out to another agent's conversation needs the reader's own project and
/// machine to write a workspace route from, and a render given neither points
/// at whatever the sender alone can name.
const NOWHERE_IN_PARTICULAR = Object.freeze({ deviceId: null, projectId: null });
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
///
/// The one reader, so a pane that draws a timeline and a pane that decides what
/// to fetch for it can never be reading two different answers.
export const digestsOf = (thread) => (thread && thread.activityDigests) || [];

/// The rows a timeline holds: its entries, or the one row an empty one shows.
/// Keyed like any other, so the reconciler takes it away the moment there is
/// something to say.
///
/// A timeline can be empty two ways, and they are not the same news. Nothing
/// was ever said — or the detail level the reader chose
/// (core/conversationDetail.js) admits none of what WAS said, which is a
/// conversation they can have back by asking for more of it. Two keys rather
/// than two bodies under one, so the reconciler swaps the row instead of
/// leaving the first answer standing.
const EMPTY_TIMELINE_ENTRY = { key: "empty", html: '<div class="thread-empty">No conversation yet.</div>' };
const NOTHING_AT_LEVEL_ENTRY = { key: "empty-level", html: '<div class="thread-empty">Nothing at this level.</div>' };

const timelineRows = ({ entries, itemCount, hiddenByLevel }) =>
  itemCount ? entries : [hiddenByLevel ? NOTHING_AT_LEVEL_ENTRY : EMPTY_TIMELINE_ENTRY];

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
    ${userMessageNavHtml(built)}
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
/// called, the offer state riding the last message, where the unread line is
/// ruled, and how much of the conversation the reader asked to see
/// (core/conversationDetail.js) — a level is a different timeline off the same
/// items, so a tick that changes only the level still has to build.
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
  unreadFrom,
  detailLevel,
  agentLabels,
  refLinks,
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
    unreadFrom ?? "",
    detailLevel || "",
    // A tracking notice names an agent, and that name comes off the feed: a
    // rename has to repaint a line that is already on screen.
    agentLabels || "",
    // What the reference resolver can answer for (#63). The list lands after
    // the first paint, and when it does, prose becomes links.
    refLinks || "",
  ].join("|");
}

const lastCallSignature = (call) => (call ? `${call.sequence}:${call.outcome || ""}` : "");

const digestsSignature = (digests) =>
  (digests || [])
    .map(
      (digest) =>
        `${digest.from_sequence}-${digest.through_sequence}:${digest.rows}:${digest.tool_calls}:${lastCallSignature(digest.last_tool_call)}`,
    )
    .join(",");

const keysSignature = (keys) => [...(keys || [])].sort().join(",");

/// The offers in hand as the paint sees them: what has been picked and not yet
/// sent, and what is being sent right now. Both are this module's own state —
/// the chips write it and the timeline reads it — so a paint that skips has to
/// be able to see it move.
export function threadOfferState(threadState = createThreadState()) {
  return threadState.snapshot();
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
  const source = section.querySelector(".thread-user-nav-source");
  source.innerHTML = userMessageTicks(built).map(userMessageTickHtml).join("");
  section.querySelector(".thread-user-nav-list").removeAttribute("data-window-start");
  syncUserMessageTicks(container);
  return framed;
}

/// The parts of a rendered thread a repaint may overwrite. The composer is
/// deliberately not among them, and neither is the revision viewer — both are
/// state the reader put there, and the poll knows nothing about either.
const REPAINTED_PARTS = [".thread-title", ".thread-user-nav", ".thread-items", ".thread-actions"];

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

/// Repaint a conversation without moving anything the reader is holding onto:
/// their place in the timeline (core/paintKeepingPlace.js) and their place
/// inside every open activity run (core/activityRunScroll.js).
export function paintThreadKeepingPlace(scroller, paint, { olderItemsPrepended = false } = {}) {
  paintKeepingPlace(scroller, () => paintRunsShowingLatest(scroller, paint), {
    opening: (element) => !element.querySelector(".review-thread"),
    policy: followConversation({ olderItemsPrepended, unreadSelector: UNREAD_LINE_SELECTOR }),
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
export function wireThreadOptions(root, submit, threadState = createThreadState()) {
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
        threadState.choose(key, picked());
        if (send) send.disabled = !picked().length;
      };
    });

    if (!send) return;
    send.onclick = () => {
      const optionIds = picked();
      if (!optionIds.length || send.disabled) return;
      threadState.beginSending(key);
      shut(true);
      Promise.resolve(submit({ messageId, optionIds })).then(
        () => {
          threadState.finishSending(key, true);
        },
        () => {
          // Refused: the offer is still the last thing said, so it goes back
          // to being answerable with the same chips still marked.
          if (threadState.finishSending(key, false)) shut(false);
        },
      );
    };
  });
}

/// Wire the sent-message lines: one press opens the whole of what was sent.
///
/// The line is a button, so the press is the browser's own — a pointer, a tap,
/// Enter, Space. The keys are taken on the way down and the browser's own
/// activation cancelled, so one press is one toggle however it arrived.
///
/// What is open is remembered outside the markup, so the next repaint of the
/// conversation draws it open rather than shutting it under the reader.
export function wireThreadSentMessages(root, threadState = createThreadState()) {
  if (!root) return;
  root.querySelectorAll(".thread-sent-preview").forEach((line) => {
    const item = line.closest(".thread-sent");
    const body = item && item.querySelector(".thread-body");
    const show = (open) => {
      line.setAttribute("aria-expanded", open ? "true" : "false");
      if (body) body.hidden = !open;
      threadState.openSentMessage(line.dataset.sentMessage, open);
    };
    const shut = () => line.getAttribute("aria-expanded") !== "true";
    line.onclick = () => show(shut());
    line.onkeydown = (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      show(shut());
    };
  });
}

/// Wire the press under a folded arrival: one press unfolds the report, a
/// second folds it back.
///
/// The fold itself is a class on the card, so opening one costs no reflow of
/// anything else and the whole report was in the markup all along. What is
/// open is remembered outside the markup, so the next repaint draws it open
/// rather than shutting it under the reader mid-read.
export function wireThreadArrivals(root, threadState = createThreadState()) {
  if (!root) return;
  root.querySelectorAll(".thread-arrival-press").forEach((press) => {
    // What the press toggles is what it says it controls: the arrival's card
    // for a report, and the issue's own body for a handed-over issue. Reading
    // it off `aria-controls` is how one press serves both without either
    // knowing about the other.
    const card = root.ownerDocument?.getElementById(press.getAttribute("aria-controls"))
      || press.closest(".thread-message")?.querySelector(".thread-comment-card");
    const show = (open) => {
      press.setAttribute("aria-expanded", open ? "true" : "false");
      card?.classList.toggle("thread-arrival-folded", !open);
      threadState.openArrival(press.dataset.arrivalMessage, open);
    };
    const folded = () => press.getAttribute("aria-expanded") !== "true";
    press.onclick = () => show(folded());
    press.onkeydown = (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      show(folded());
    };
  });
}

export function wireThreadAttachments(root, load, threadState = createThreadState()) {
  if (!root) return;
  const dataUrlFor = (path) => {
    const held = threadState.attachment(path);
    if (held) return Promise.resolve(held);
    return threadState.loadAttachment(path, async () => {
      const attachment = await load(path);
      const dataUrl = `data:${attachment.mime || "application/octet-stream"};base64,${attachment.content_b64 || ""}`;
      threadState.rememberAttachment(path, dataUrl);
      return dataUrl;
    });
  };

  /// Which of the two failures this was, and what the reader is shown for it.
  ///
  /// A bridge that refused the path is final: the figure says so, the refusal is
  /// remembered, and no repaint asks again — the bytes are not coming. A wire
  /// that was not there refused nothing; the request never arrived anywhere, so
  /// the figure keeps saying "loading" and the path is asked for again once the
  /// device is back (#30). Telling them apart is the one rule
  /// core/transientRead.js owns, because a second reading of it would be a
  /// second answer about the same failure.
  const failedToLoad = (path, image, error) => {
    const figure = image?.closest(".thread-attachment-figure");
    const transient = isTransientTransportError(error);
    // Recorded, because "my attachments show unavailable" was half of the report
    // this came from and the answer turns entirely on which of the two this was.
    // A figure the reader says is wrong is then one line in Settings →
    // Diagnostics: what the fetch was refused with, and what that was read as.
    recordConnectionDiagnostic(threadState.ownerId || "attachments", "attachment-failed", {
      state: transient ? "deferred" : "refused",
      refusal: (error && error.message) || String(error || ""),
    });
    if (transient) {
      threadState.deferAttachment(path);
      figure?.classList.add("waiting");
      return;
    }
    threadState.rememberAttachment(path, null);
    figure?.classList.add("unavailable");
  };

  root.querySelectorAll("img.thread-attachment-image").forEach((image) => {
    const path = image.dataset.attachmentPath;
    // A picture already showing, one already asked for and refused, and one
    // waiting on a wire that is not there are all settled for now: asking again
    // on every poll would be a request a second and a half for bytes that are
    // not coming back on this render.
    if (!path || image.getAttribute("src") || threadState.attachment(path) === null) return;
    if (threadState.attachmentDeferred(path)) return;
    dataUrlFor(path).then(
      (dataUrl) => {
        image.setAttribute("src", dataUrl);
      },
      (error) => failedToLoad(path, image, error),
    );
  });

  root.querySelectorAll("button.thread-attachment-preview").forEach((preview) => {
    preview.onclick = async () => {
      const image = preview.querySelector("img.thread-attachment-image");
      const path = image?.dataset.attachmentPath;
      if (!path) return;
      try {
        const dataUrl = image.getAttribute("src") || await dataUrlFor(path);
        if (dataUrl) openThreadAttachmentLightbox(preview, { src: dataUrl, alt: image.alt });
      } catch (error) {
        failedToLoad(path, image, error);
      }
    };
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

/// Whether a press is the browser's rather than the app's. A middle click, or
/// a click held with a modifier, means "open this somewhere else" — another
/// tab, another window — and a real link already knows how.
export const pressIsTheBrowsers = (event) =>
  event.button > 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey;

/**
 * Wire the reference chips under the messages of a conversation.
 * `openLink(link)` navigates the app to the work item a chip names.
 */
export function wireThreadLinks(root, openLink) {
  if (!root) return;
  root.querySelectorAll(".thread-reference").forEach((chip) => {
    const link = linkFromDataset(chip.dataset);
    chip.onclick = (event) => {
      event.preventDefault();
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
/// `onSubmit(body, attachments)` does the transport. `onInterrupt` stops the
/// active turn when the empty composer is showing its stop control. `upload` (with the
/// `readAttachments`/`writeAttachments` draft pair) turns the box into one that
/// takes files; without it the composer is the plain text box it always was.
///
/// Returns a controller: `setCanInterrupt(flag)` moves the send between its two
/// shapes in place, for a surface whose poll can change the answer under a box
/// somebody is typing in.
function mountViewingContext(root, inputId, viewingContext) {
  const tray = root.querySelector(`#${composerPartIds(inputId).context}`);
  const paint = (context = viewingContext?.snapshot?.()) => {
    if (!tray) return;
    const expanded = new Set([...tray.querySelectorAll("details[data-context-group][open]")].map((detail) => detail.dataset.contextGroup));
    const holder = document.createElement("div");
    holder.innerHTML = viewingContextChipsHtml(context, { removable: true });
    holder.querySelectorAll("details[data-context-group]").forEach((detail) => { detail.open = expanded.has(detail.dataset.contextGroup); });
    setMotionRowHtml(tray, holder.innerHTML);
    tray.querySelectorAll("details[data-context-group]").forEach((detail) => { detail.open = expanded.has(detail.dataset.contextGroup); });
    tray.querySelectorAll(":scope > :not([data-motion-snapshot]) button").forEach((button) => {
      button.onclick = () => {
        const group = button.closest("[data-context-indices]");
        if (group) viewingContext.removeMany(group.dataset.contextIndices.split(",").map(Number));
        else viewingContext?.remove?.(Number(button.closest("[data-context-index]").dataset.contextIndex));
      };
    });
  };
  const unsubscribe = viewingContext?.subscribe?.(paint);
  paint();
  return unsubscribe;
}

export function wireThreadComposer(root, {
  ids,
  onSubmit,
  onInterrupt,
  readDraft,
  writeDraft,
  onError,
  afterSubmit,
  upload,
  readAttachments,
  writeAttachments,
  submissionOwnsDraft = false,
  viewingContext = null,
}) {
  if (!root) return null;
  const input = root.querySelector(`#${ids.input}`);
  let send = root.querySelector(`#${ids.send}`);
  const control = root.querySelector(`#${composerPartIds(ids.input).sendControl}`);
  const hint = ids.hint ? root.querySelector(`#${ids.hint}`) : null;
  if (!input || !send) return null;
  const unsubscribeContext = mountViewingContext(root, ids.input, viewingContext);

  const say = (message) => {
    if (hint) hint.textContent = message;
  };
  let canInterrupt = !!send?.classList.contains("is-stop");
  let submitting = false;
  let blocked = false;
  let tray = null;
  const hasDraft = () => input.value.trim() !== "" || !!(tray && !tray.isEmpty());
  const paintAction = () => {
    if (!control || submitting) return;
    control.innerHTML = sendControlHtml({ sendId: ids.send, canInterrupt, hasDraft: hasDraft() });
    wireSendControl();
    setPressable(true);
  };
  tray = upload
    ? mountComposerAttachments(root, {
        ids,
        upload,
        readAttachments,
        writeAttachments,
        onError: say,
        onChange: () => queueMicrotask(paintAction),
      })
    : null;

  input.value = readDraft();
  input.oninput = () => {
    writeDraft(input.value);
    if (!blocked) say("");
    paintAction();
  };
  const fitToText = autoGrow(input);

  const setPressable = (pressable) => {
    const disabled = !pressable || blocked;
    send.disabled = disabled;
  };

  // eslint-disable-next-line complexity -- ratchet: this callback is at 14, cap 10 — reduce it, then drop this line
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
    setPressable(false);
    submitting = true;
    try {
      const result = await onSubmit(body, tray ? tray.attachments() : []);
      if (!submissionOwnsDraft) writeDraft("");
      input.value = "";
      fitToText();
      if (tray) tray.clear();
      submitting = false;
      setPressable(true);
      paintAction();
      if (afterSubmit) afterSubmit(result);
    } catch (error) {
      // The text and the files stay put: a failed send must never cost the user
      // their words, and re-picking the files would be worse.
      submitting = false;
      setPressable(true);
      paintAction();
      if (onError) onError(error);
    }
  };

  /// Take hold of whichever shape the send control is wearing. Called again
  /// after a swap, because the buttons it wires are new elements.
  const wireSendControl = () => {
    send = root.querySelector(`#${ids.send}`);
    if (!send) return;
    send.onclick = async () => {
      if (send.dataset.action !== "stop") return submit();
      if (send.disabled || !onInterrupt) return;
      setPressable(false);
      submitting = true;
      try {
        await onInterrupt();
      } catch (error) {
        if (onError) onError(error);
      } finally {
        submitting = false;
        paintAction();
      }
    };
  };

  /// Record whether the active turn can be stopped. A request already in
  /// flight keeps its disabled control until it settles, then paints the latest
  /// turn and draft state.
  const setCanInterrupt = (wanted) => {
    const next = !!wanted;
    if (!control || next === canInterrupt) return;
    canInterrupt = next;
    paintAction();
  };

  wireSendControl();
  paintAction();
  input.onkeydown = (event) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      submit();
    }
  };

  const setBlocked = (wanted, message = "") => {
    blocked = !!wanted;
    setPressable(!submitting);
    if (!submitting) {
      say(blocked ? message : "");
    }
  };

  return {
    setCanInterrupt,
    setBlocked,
    /// Put files in the tray as a drop on the box would — for a drop that
    /// landed somewhere else and was carried here.
    addFiles: (files) => {
      if (tray) tray.addFiles(files);
    },
    dispose: () => unsubscribeContext?.(),
  };
}
