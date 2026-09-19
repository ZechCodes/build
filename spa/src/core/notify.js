// Global error/success notifications: a fixed stack of expandable, dismissable
// notices. Failures persist until dismissed (promoting gitPane's error-latching
// honesty pattern); successes auto-dismiss. Pure list/markup layer is
// unit-tested; the DOM layer stays thin.

import { esc } from "./text.js";

let nextNoticeId = 1;

/** Returns a NEW list with the notice appended — or, when an identical
 *  kind+summary+detail notice already exists, with that notice's count
 *  incremented (dedupe for repeating failures like a polling RPC). */
export function addNotice(list, { kind, summary, detail = "" }) {
  const existing = list.find((n) => n.kind === kind && n.summary === summary && n.detail === detail);
  if (existing) return list.map((n) => (n === existing ? { ...n, count: n.count + 1 } : n));
  return [...list, { id: nextNoticeId++, kind, summary, detail, count: 1 }];
}

/** Returns a new list without the notice with that id. */
export function dismissNotice(list, id) {
  return list.filter((n) => n.id !== id);
}

/** One notice's markup. Errors announce as role=alert, successes as
 *  role=status. Detail (full error text) starts collapsed. All strings
 *  escaped. */
export function noticeHtml(notice) {
  const role = notice.kind === "error" ? "alert" : "status";
  const multiplier = notice.count > 1 ? ` ×${notice.count}` : "";
  const expandButton = notice.detail
    ? `<button class="notice-expand" aria-label="Show details"><span class="disclosure-caret" aria-hidden="true">▾</span></button>`
    : "";
  const detailHtml = notice.detail ? `<pre class="notice-detail" hidden>${esc(notice.detail)}</pre>` : "";
  return (
    `<div class="notice ${esc(notice.kind)}" data-notice="${esc(notice.id)}" role="${role}">` +
    `<div class="notice-head">` +
    `<span class="notice-summary">${esc(notice.summary)}${multiplier}</span>` +
    expandButton +
    `<button class="notice-x" aria-label="Dismiss">×</button>` +
    `</div>` +
    detailHtml +
    `</div>`
  );
}

// ---- DOM layer (thin, untested — vitest runs without a DOM) ----

const SUCCESS_AUTO_DISMISS_MS = 4000;

let notices = [];
const successTimers = new Map();
// The notices whose detail the reader has opened. Kept beside the list, because
// the failures that repeat are exactly the ones somebody opens to read.
const expandedNotices = new Set();

function noticesContainer() {
  let container = document.getElementById("notices");
  if (!container) {
    container = document.createElement("div");
    container.id = "notices";
    document.body.appendChild(container);
  }
  return container;
}

function dismissById(id) {
  const timer = successTimers.get(id);
  if (timer !== undefined) {
    clearTimeout(timer);
    successTimers.delete(id);
  }
  expandedNotices.delete(id);
  notices = dismissNotice(notices, id);
  repaintNotices();
}

/** Show the notice's detail as open or shut, following what the reader chose. */
function applyDisclosure(noticeEl, id) {
  const detail = noticeEl.querySelector(".notice-detail");
  const expand = noticeEl.querySelector(".notice-expand");
  if (!detail || !expand) return;
  const open = expandedNotices.has(id);
  detail.hidden = !open;
  expand.querySelector(".disclosure-caret").textContent = open ? "▴" : "▾";
  expand.setAttribute("aria-label", open ? "Hide details" : "Show details");
}

function wireNotice(noticeEl) {
  const id = Number(noticeEl.dataset.notice);
  noticeEl.querySelector(".notice-x").onclick = () => dismissById(id);
  const expand = noticeEl.querySelector(".notice-expand");
  if (expand)
    expand.onclick = () => {
      if (expandedNotices.has(id)) expandedNotices.delete(id);
      else expandedNotices.add(id);
      applyDisclosure(noticeEl, id);
    };
  applyDisclosure(noticeEl, id);
}

/** The same notices in the same order: nothing was added or dismissed, so the
 *  only thing that can have moved is a repeat's count. */
const sameNoticesOnScreen = (container) => {
  const painted = [...container.querySelectorAll(".notice")];
  return painted.length === notices.length && painted.every((element, index) => Number(element.dataset.notice) === notices[index].id);
};

function repaintNotices() {
  const container = noticesContainer();
  // A failure repeating is the case this stack is built for, and it is also the
  // case a reader is most likely to have opened. The count is written into the
  // notice that is already there, so the detail they are reading — and any text
  // they are selecting in it — is never taken away.
  if (sameNoticesOnScreen(container)) {
    container.querySelectorAll(".notice").forEach((noticeEl, index) => {
      const notice = notices[index];
      const summary = noticeEl.querySelector(".notice-summary");
      const text = notice.count > 1 ? `${notice.summary} ×${notice.count}` : notice.summary;
      if (summary.textContent !== text) summary.textContent = text;
    });
    return;
  }
  container.innerHTML = notices.map(noticeHtml).join("");
  container.querySelectorAll(".notice").forEach(wireNotice);
}

/** Show a persistent error notice (collapsed summary, expandable detail);
 *  stays until the user dismisses it. */
export function notifyError(summary, detail = "") {
  notices = addNotice(notices, { kind: "error", summary, detail });
  repaintNotices();
}

/** Show a transient success notice; auto-dismisses after a few seconds. */
export function notifySuccess(summary) {
  notices = addNotice(notices, { kind: "success", summary });
  repaintNotices();
  const notice = notices.find((n) => n.kind === "success" && n.summary === summary && n.detail === "");
  if (notice && !successTimers.has(notice.id)) {
    successTimers.set(
      notice.id,
      setTimeout(() => dismissById(notice.id), SUCCESS_AUTO_DISMISS_MS),
    );
  }
}

/** Clear every notice and pending timer (used on teardown and in tests). */
export function dismissAllNotices() {
  successTimers.forEach((timer) => clearTimeout(timer));
  successTimers.clear();
  expandedNotices.clear();
  notices = [];
  const container = document.getElementById("notices");
  if (container) container.innerHTML = "";
}
