// The account archive's pure model: one list of everything that has ended,
// across every project, and the record a row shows when it is opened.
//
// The archive is the user's, not a project's — Done archives an inbox entry,
// and this is where the entry goes. It reads `archived.list`'s items[], the
// same two work items the feed speaks (branches and issues), so a row here is
// the row you last saw in the inbox, told in the past tense.
//
// Nothing here is actionable: an archived record is history, and history has no
// buttons. No DOM, no app imports — views/archive.js wires these.

import { esc } from "./text.js";

/** How the work ended, in words. The token is a run state, an issue state, or
 *  the bare `archived` a finished checkout leaves behind. A token this client
 *  has never heard of is shown as it came: a new ending must read as SOMETHING,
 *  never as nothing. */
const STATE_LABEL = {
  archived: "Archived",
  merged: "Merged",
  abandoned: "Abandoned",
  failed: "Failed",
  interrupted: "Interrupted",
  idle_unreported: "Went idle",
  approved: "Implemented",
  review: "Left in review",
  plan_review: "Left in review",
  drafting: "Left drafting",
  building: "Left building",
  stage_gate: "Left at a stage gate",
  blocked: "Left blocked",
  created: "Never started",
};

/** What `worktree.finish` did with the checkout. */
const ACTION_LABEL = {
  delete: "Checkout deleted",
  cleanup: "Checkout removed, branch kept",
  push: "Pushed, then removed",
  merge: "Merged, then removed",
};

const text = (value) => {
  if (value === null || value === undefined) return null;
  const trimmed = String(value).trim();
  return trimmed ? trimmed : null;
};

const count = (value) => (Number.isFinite(value) ? Math.max(0, Math.floor(value)) : null);

const ms = (iso) => {
  const parsed = Date.parse(iso || "");
  return Number.isFinite(parsed) ? parsed : null;
};

/** The day it ended, in the reader's own locale. A record whose stamp never got
 *  written says so, rather than showing an invented date. */
export function archiveDateLabel(iso) {
  const at = ms(iso);
  if (at === null) return "date unknown";
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(new Date(at));
}

const KIND_LABEL = { issue: "Issue", branch: "Branch" };

// eslint-disable-next-line complexity -- ratchet: toRow is at 12, cap 10 — reduce it, then drop this line
function toRow(item, index) {
  const kind = item.kind === "issue" ? "issue" : "branch";
  const branch = text(item.branch);
  const state = text(item.state);
  const action = text(item.action);
  const finishedAt = text(item.finished_at);
  return {
    key: text(item.run_id) || text(item.issue_id) || text(item.worktree_id) || `row-${index}`,
    kind,
    kindLabel: KIND_LABEL[kind],
    title: text(item.title) || branch || "(untitled)",
    project: text(item.project) || "",
    projectId: text(item.project_id),
    branch,
    state,
    stateLabel: state === null ? "Ended" : STATE_LABEL[state] || state,
    action,
    actionLabel: action === null ? null : ACTION_LABEL[action] || action,
    finishedAt,
    finishedMs: ms(finishedAt),
    finishedLabel: archiveDateLabel(finishedAt),
    runId: text(item.run_id),
    issueId: text(item.issue_id),
    stages: count(item.stages),
    worktreeId: text(item.worktree_id),
    worktreePath: text(item.worktree_path),
    headSha: text(item.head_sha),
    upstream: text(item.upstream),
    unpushed: count(item.unpushed),
    dirtyFiles: count(item.dirty_files),
  };
}

/** The order the archive reads in: newest first, and a record missing its stamp
 *  keeps its place at the end, because a record with no date is still a record.
 *  Stated once, because the merge across devices orders the same way. */
export const newestFirst = (first, second) =>
  (second.finishedMs ?? Number.NEGATIVE_INFINITY) - (first.finishedMs ?? Number.NEGATIVE_INFINITY);

/** Everything one bridge filed away, newest first. Rows that are not objects
 *  are dropped. */
export function archiveRows(payload) {
  const items = Array.isArray(payload?.items) ? payload.items : [];
  return items
    .filter((item) => item && typeof item === "object")
    .map(toRow)
    .sort(newestFirst);
}

const factRow = (label, value) =>
  value === null || value === undefined || value === ""
    ? ""
    : `<div class="row"><span class="k">${esc(label)}</span><span class="v">${esc(value)}</span></div>`;

const plural = (n, singular) => `${n} ${singular}${n === 1 ? "" : "s"}`;

/** One archived record, stated and nothing more. */
export function archiveRecordHtml(row) {
  const facts = [
    factRow("Project", row.project),
    factRow("Kind", row.kindLabel),
    factRow("Branch", row.branch),
    factRow("Ended", row.stateLabel),
    factRow("Finished", row.finishedLabel),
    factRow("Finish action", row.actionLabel),
    row.stages === null ? "" : factRow("Stage plans", plural(row.stages, "stage")),
    factRow("Checkout", row.worktreePath),
    factRow("HEAD", row.headSha),
    factRow("Upstream", row.upstream),
    row.unpushed === null ? "" : factRow("Unpushed", plural(row.unpushed, "commit")),
    row.dirtyFiles === null ? "" : factRow("Uncommitted", plural(row.dirtyFiles, "file")),
    factRow("Record", row.runId || row.issueId || row.worktreeId),
  ].join("");
  return `<div class="panel archive-record">${facts}</div>`;
}

/** One archived row, and its record when it is the open one. */
export function archiveRowHtml(row, ui = {}) {
  const open = ui.openKey === row.key;
  return `<div class="card quiet archive-row" data-key="${esc(row.key)}" role="button" tabindex="0" aria-expanded="${open}">
    <div class="top"><span class="title">${esc(row.title)}</span><span class="chip work">${esc(row.kindLabel)}</span></div>
    <div class="meta"><span>${esc(row.project)}</span><span>·</span><span>${esc(row.stateLabel)}</span><span>·</span><span>${esc(
      row.finishedLabel,
    )}</span>${row.branch ? `<span>·</span><span>${esc(row.branch)}</span>` : ""}</div>
  </div>${open ? archiveRecordHtml(row) : ""}`;
}

export function archiveListHtml(rows, ui = {}) {
  if (!rows.length) {
    return '<div class="empty">Nothing is archived yet. Work you mark Done lands here.</div>';
  }
  return rows.map((row) => archiveRowHtml(row, ui)).join("");
}

/** The account's own pages, above whichever one is open. Devices live inside
 *  Settings, so there are two. */
const ACCOUNT_PAGES = [
  { page: "settings", label: "Settings" },
  { page: "archive", label: "Archive" },
];

export function accountNavHtml(current) {
  const page = current === "archive" ? "archive" : "settings";
  return `<div class="tabs account-nav">${ACCOUNT_PAGES.map(
    (entry) =>
      `<div class="t${entry.page === page ? " active" : ""}" data-page="${entry.page}" role="button" tabindex="0">${entry.label}</div>`,
  ).join("")}</div>`;
}
