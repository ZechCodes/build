// Global compose: the one box that takes what you want to get done, wherever
// you are standing.
//
// Capture first, route after (Decisions §Capture and router). Submitting hands
// the text to the daemon's router and decides nothing — the only routing
// decision this box offers is the advanced panel, for the times you already
// know the destination.
//
// This module is the pure half: what the client holds when it cannot send, how
// a held capture reaches the daemon, when `c` is the user asking for the box,
// and the markup. The wiring is core/composeView.js.

import { esc } from "./text.js";

/** Where captures wait out an offline device. Browser-scoped, like every other
 *  thing this client keeps for itself. */
export const CAPTURE_QUEUE_KEY = "build.captureQueue";

/** How long a capture stays on the inbox after its route settles. Routing is
 *  visible and reversible, and a row that vanishes the instant it is decided is
 *  neither. */
export const ROUTED_LINGER_MS = 120000;

/** How much of the text a row's title shows — the width the daemon cuts its
 *  own capture titles to, so a held capture and a stored one read alike. */
const TITLE_WIDTH = 80;

// ---- what the client holds ---------------------------------------------------

/** One capture the client is holding: the text, an id of its own making, when
 *  it was said, and the machine it is going to. The daemon mints the real id
 *  when it takes it. */
export function queuedCapture(text, { id, createdAt, deviceId = null }) {
  return { id, text, createdAt, deviceId };
}

const isQueued = (entry) =>
  !!entry && typeof entry.id === "string" && typeof entry.text === "string" && typeof entry.createdAt === "string";

/** The queue as the device kept it. Never throws: an unreadable or corrupt
 *  store reads as holding nothing, which is what it is. */
export function loadCaptureQueue(storage = localStorage) {
  try {
    const parsed = JSON.parse(storage.getItem(CAPTURE_QUEUE_KEY) || "[]");
    return Array.isArray(parsed) ? parsed.filter(isQueued) : [];
  } catch {
    return [];
  }
}

/** Persist the queue, and answer it so a caller can render without re-reading.
 *  A device that refuses to store it keeps working — the queue then lasts the
 *  session, which is still longer than the reconnect it is waiting on. */
export function saveCaptureQueue(queue, storage = localStorage) {
  try {
    storage.setItem(CAPTURE_QUEUE_KEY, JSON.stringify(queue));
  } catch {
    /* private mode: the queue lives as long as the tab does */
  }
  return queue;
}

/** The queue without one entry. Pure: the caller owns when the change is kept. */
export function withoutQueued(queue, id) {
  return queue.filter((entry) => entry.id !== id);
}

/**
 * Hand the held captures to the daemon, oldest first.
 *
 * Stops at the first one that will not go: the device is either there or it is
 * not, and sending the rest past a failure would reorder what the user said.
 * Answers what went (with the record the daemon made for each) and what is left
 * to try again on the next reconnect.
 */
export async function flushCaptureQueue(queue, send) {
  const sent = [];
  for (const [index, queued] of queue.entries()) {
    try {
      sent.push({ queued, capture: await send(queued.text) });
    } catch {
      return { sent, remaining: queue.slice(index) };
    }
  }
  return { sent, remaining: [] };
}

// ---- the shortcut ------------------------------------------------------------

const EDITABLE_TAGS = new Set(["INPUT", "TEXTAREA", "SELECT"]);

/** Whether this keypress is the user asking for the compose box.
 *
 *  A bare `c`, and only where the letter is not already spoken for: not inside
 *  a field, not inside a terminal (where every key belongs to the PTY), and
 *  never with a modifier, which is the browser's or the OS's. */
export function composeShortcutFires(event) {
  if (!event || event.key !== "c" || event.metaKey || event.ctrlKey || event.altKey) return false;
  const target = event.target || {};
  if (EDITABLE_TAGS.has(target.tagName) || target.isContentEditable) return false;
  return !(typeof target.closest === "function" && target.closest(".term-screen, .console-pane"));
}

// ---- a capture as an inbox row -----------------------------------------------

/** The first line a row can show of what was said. */
export function captureTitle(text) {
  const first = String(text || "")
    .split("\n")
    .map((line) => line.trim())
    .find((line) => line !== "");
  if (!first) return "";
  return first.length <= TITLE_WIDTH ? first : `${first.slice(0, TITLE_WIDTH).trimEnd()}…`;
}

/** Why a capture needs the user, in the daemon's own tokens: an unanswered
 *  question first (the router is asking), then a route that gave up. */
function captureUnreadReason(capture) {
  if (capture.question && !capture.question.answer) return "router_question";
  return capture.state === "failed" ? "routing_failed" : null;
}

/**
 * A capture record — `capture.create`, `capture.get`, or one this client is
 * still holding — as an inbox row, in the shape `board.list` gives the captures
 * it carries. One row vocabulary, whichever side the copy came from.
 */
// eslint-disable-next-line complexity -- ratchet: captureRow is at 11, cap 10 — reduce it, then drop this line
export function captureRow(capture, { projectName = "", deviceId = null } = {}) {
  const routing = capture.routing || null;
  const reason = captureUnreadReason(capture);
  return {
    kind: "capture",
    capture_id: capture.id,
    // Which machine this row is on, stamped the way the feed stamps its own
    // rows: a capture the client is holding is not on any device's board yet,
    // and the rail narrowed to one machine still has to know whose it is.
    deviceId,
    project_id: routing ? routing.project_id : "",
    project: projectName,
    branch: routing && routing.kind === "branch" ? routing.target_id : null,
    issue_id: routing && routing.kind === "issue" ? routing.target_id : null,
    title: captureTitle(capture.text),
    text: capture.text,
    state: capture.state,
    created_at: capture.created_at,
    resume_at: capture.created_at,
    unread: reason !== null,
    unread_count: reason === null ? 0 : 1,
    unread_reason: reason,
    working: capture.state === "routing",
    working_time: null,
    agents: [],
    stat: null,
    can_finish: false,
    muted: false,
    worktree_path: null,
    worktree_id: null,
    run_id: null,
    primary: false,
    routing,
    question: capture.question || null,
  };
}

/** A capture the client is still holding, as its own row: it is on its way to
 *  the daemon, which is a kind of working. */
export function queuedCaptureRow(queued) {
  return {
    ...captureRow(
      { id: queued.id, text: queued.text, created_at: queued.createdAt, state: "queued" },
      { deviceId: queued.deviceId || null },
    ),
    working: true,
  };
}

/** The feed's rows plus the ones this client is carrying, deduped by capture
 *  id — the daemon's copy is the one that wins, because it is the record. */
export function mergeCaptureRows(items, extraRows) {
  const known = new Set((items || []).filter((row) => row.kind === "capture").map((row) => row.capture_id));
  return [...(items || []), ...(extraRows || []).filter((row) => !known.has(row.capture_id))];
}

/** Whether a routed capture has been on screen long enough. Only a settled
 *  route ages out: unfinished business stays until it is finished. */
export function routedCaptureExpired(tracked, nowMs) {
  if (!tracked || tracked.state !== "routed" || !tracked.settledAt) return false;
  return nowMs - tracked.settledAt > ROUTED_LINGER_MS;
}

// ---- the box -----------------------------------------------------------------

/** The question the composer asks when it is not naming a machine: on the box
 *  at rest, and in the open box while this client cannot say where the capture
 *  is going. One sentence, minted once. */
const PLAIN_QUESTION = "What do you want to get done?";

/** The compose affordance at rest: one line, the whole question. */
export function composePromptHtml() {
  return `<button class="compose-prompt" id="compose-open" type="button">${PLAIN_QUESTION}</button>`;
}

/**
 * The open box. `placeholder` is the line it asks with and `note` is what the
 * client wants to say about where this is going (queued while the device is
 * away) — both computed by the caller, which is the one that knows which
 * machine this capture is for; `advanced` is the manual panel's markup,
 * rendered only while its disclosure is open.
 */
// eslint-disable-next-line complexity -- ratchet: composeBoxHtml is at 13, cap 10 — reduce it, then drop this line
export function composeBoxHtml({ value = "", placeholder, note = "", error = "", busy = false, advanced = "" } = {}) {
  return `<div class="compose-box">
    <textarea id="compose-text" rows="3" placeholder="${esc(placeholder)}"
      aria-label="${esc(placeholder)}">${esc(value)}</textarea>
    <div class="compose-row">
      <button class="compose-disclose" id="compose-advanced" type="button" aria-expanded="${advanced ? "true" : "false"}">
        ${advanced ? "▾" : "▸"} I know where this goes</button>
      <button class="btn mini" id="compose-cancel" type="button">Cancel</button>
      <button class="btn mini primary" id="compose-send" type="button"${busy ? " disabled" : ""}>${busy ? "sending…" : "Capture"}</button>
    </div>
    ${note ? `<div class="compose-note dim">${esc(note)}</div>` : ""}
    <div class="warn compose-error"${error ? "" : " hidden"}>${esc(error)}</div>
    ${advanced}
  </div>`;
}

// ---- the advanced panel ------------------------------------------------------
//
// The manual flow, for the times you already know the destination. It bypasses
// the router and speaks the same two verbs the router's own tools do, so what
// it makes is the same kind of thing a routed capture makes.

/** The branches a project already has, once each, as the panel offers them. */
export function branchOptions(items, projectId) {
  const names = (items || [])
    .filter((row) => row.kind === "branch" && row.project_id === projectId && row.branch)
    .map((row) => row.branch);
  return [...new Set(names)];
}

/** The call a manual route makes. An issue is inert by contract — the record
 *  exists and nothing runs until the first message; a branch is dispatched in
 *  one call, which is worktree, agent and first message together. */
export function manualRoute({ kind, projectId, text, branch = "", agentParams = {} }) {
  if (kind === "branch") {
    const named = String(branch || "").trim();
    return {
      method: "branch.dispatch",
      params: { project_id: projectId, instruction: text, ...(named ? { branch: named } : {}), ...agentParams },
    };
  }
  return {
    method: "issue.create",
    params: { goal: text, project_id: projectId, dispatch: false, ...agentParams },
  };
}

/** Where a manual route lands: what it made, opened — or nowhere, when the
 *  reply names nothing to open. The daemon cuts a branch with its state lock
 *  released and answers once the git lands, so the reply can name the work
 *  after this browser has stopped waiting for it; the board carries the row
 *  either way. */
export function manualRouteDestination(kind, created, projectId) {
  const made = created || {};
  const project = made.project_id || projectId;
  if (kind === "branch") {
    return made.branch ? { name: "branch", projectId: project, branch: made.branch, tab: "changes" } : null;
  }
  const issueId = made.issue_id || made.plan_id;
  return issueId ? { name: "issue", projectId: project, id: issueId } : null;
}

/** The line the open box asks with: the machine the capture is going to, when
 *  this client can name it, and the plain question when it cannot. */
export const composePlaceholder = (deviceName) => (deviceName ? `Capture on ${deviceName}` : PLAIN_QUESTION);

/** What the box says about a capture it cannot send yet: which machine it is
 *  waiting for, and that the text is kept meanwhile — the difference between a
 *  queue and a loss. A client that cannot name the machine says whose it is. */
export function composeOfflineNote(queuedCount, deviceName) {
  if (!queuedCount) return `${awayFrom(deviceName)} — this is kept here and sent when it is back.`;
  const waitingFor = deviceName || "your device";
  return queuedCount === 1 ? `1 capture is waiting for ${waitingFor}.` : `${queuedCount} captures are waiting for ${waitingFor}.`;
}

/** How every line about a machine that cannot take work starts — named when
 *  this client can name it, and whose it is when it cannot. */
const awayFrom = (deviceName) => `${deviceName || "Your device"} is away`;

/** What the manual panel says instead of creating: the box beside it takes the
 *  text whatever happens, which is the way out this offers. */
export const composeManualAwayNote = (deviceName) =>
  `${awayFrom(deviceName)} — capture it instead and it will be routed when it is back.`;
