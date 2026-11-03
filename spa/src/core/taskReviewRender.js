import { esc } from "./text.js";
import { directoryTabsHtml } from "./workspaceDirectoryTabs.js";
import { hashFromRoute } from "./router.js";
import { ageText } from "./trackerChips.js";
import { taskReviewStatusLabel } from "./taskReviewSummaryRender.js";

export const reviewSnapshot = (review, id) => review?.snapshots.find((snapshot) => snapshot.id === id) || review?.snapshots.at(-1) || null;
export const reviewDirectory = (snapshot, id) => snapshot?.directories.find((directory) => directory.id === id) || snapshot?.directories[0] || null;
export const defaultReviewView = (directory) => directory?.status === "not_git" ? "files" : "changes";

const legacyStatusHtml = (review) => `<span class="task-review-summary">${review?.state === "completed" ? "Completed" : "Open"}</span>`;
const reviewTitleHtml = (task, dedicated) => `<div class="task-review-title"><span>Review${task.number ? ` #${Number(task.number)}` : ""}</span><h1${dedicated ? ' class="task-page-title"' : ""}>${esc(task.title || "Review")}</h1></div>`;
const sinceCountHtml = (count) => Number.isSafeInteger(count) ? `<span>${count} commits since your review</span>` : "";
function workTaskHtml(review, task, scope) {
  const id = task.links?.parent_task_id || review?.for_task_id;
  return id ? `<a href="${esc(hashFromRoute({ name: "trackerTask", ...scope, taskId: id }))}">Work task</a>` : "";
}
function headerActionsHtml(snapshot, support, isPr) {
  const reviewer = snapshot ? '<button class="btn" data-review-reviewer>Choose reviewer</button>' : "";
  const refresh = support.get && !isPr ? '<button class="btn" data-review-refresh>Refresh</button>' : "";
  return `<div data-review-header-actions>${reviewer}${refresh}</div>`;
}
export function reviewHeadHtml(review, snapshot, support, { task = {}, scope = {}, sinceCount = null, dedicated = false } = {}) {
  const picker = snapshot ? snapshotPickerHtml(review, snapshot) : "";
  const isPr = review?.mode === "pull_request";
  const status = isPr ? reviewStatusHtml(review) : legacyStatusHtml(review);
  const facts = isPr ? reviewBranchesHtml(review, snapshot) : completionHtml(review);
  return `${reviewTitleHtml(task, dedicated)}<div class="task-review-head">${status}${picker}${sinceCountHtml(sinceCount)}${workTaskHtml(review, task, scope)}<button class="btn task-review-phone-commits" data-review-commits-open type="button" aria-expanded="false">Commits</button></div><div class="task-review-header-facts">${facts}</div>${headerActionsHtml(snapshot, support, isPr)}`;
}

const completionHtml = (review) => review?.completion ? `<p class="task-review-completion">Completed: ${esc(review.completion.description)}</p>` : "";
const reviewStatusHtml = (review) => `<span class="task-review-summary" data-review-pr-status="${esc(review.pull_request?.status)}">${esc(taskReviewStatusLabel(review.pull_request?.status))}</span>`;
const revisionTime = (value) => {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
};
const snapshotPickerHtml = (review, snapshot) => `<label><span class="sr-only">Revision</span><select data-review-snapshot aria-label="Revision">${review.snapshots.map((row) =>
  `<option value="${esc(row.id)}"${row.id === snapshot.id ? " selected" : ""}>Revision ${row.number} of ${review.snapshots.at(-1).number} · ${esc(revisionTime(row.created_at))}</option>`).join("")}</select></label>`;
const shortBranch = (ref = "") => ref.replace(/^refs\/heads\//, "");

function reviewBranchesHtml(review, snapshot) {
  return `<div class="task-review-branches">${(review.bindings || []).map((binding) => bindingFactsHtml(binding, snapshot)).join("")}</div>`;
}

function bindingFactsHtml(binding, snapshot) {
  const directory = snapshot?.directories.find((row) => row.id === binding.directory_id);
  const base = directory?.base?.name || shortBranch(binding.base_branch_ref);
  const branch = directory?.branch || shortBranch(binding.dedicated_branch_ref);
  return `<p>${esc(directory?.name || binding.directory_id)} · <code>${esc(base)} ← ${esc(branch)}</code></p>`;
}

export function reviewDirectoryHtml(snapshot, directory, view) {
  if (!directory) return '<p class="sub">No directories in this revision.</p>';
  const tabs = directoryTabsHtml(snapshot.directories.map((row) => ({ sourceId: row.id, label: row.name, current: row.id === directory.id })));
  const viewTabs = ["changes", "files"].map((name) => `<button type="button" class="btn${view === name ? " primary" : ""}" data-review-view="${name}" aria-pressed="${view === name}">${name === "changes" ? "Changes" : "Files"}</button>`).join("");
  return `${tabs}<div class="task-review-facts">${directoryFacts(directory)}</div><div class="task-review-views" role="group" aria-label="Directory view">${viewTabs}</div>`;
}

function directoryFacts(directory) {
  const dirty = directory.uncommitted_files ? `<p>${Number(directory.uncommitted_files)} uncommitted files not in this review</p>` : "";
  if (directory.status === "not_git") return '<p>Not a Git repository</p><p>Live files — not saved with this review</p>';
  if (directory.status === "unavailable") return `<p>Source unavailable</p><p>${esc(directory.reason || "")}</p>`;
  if (directory.status === "no_commits") return `<p>No commits yet</p>${dirty}`;
  return `${gitFacts(directory)}${dirty}`;
}

function gitFacts(directory) {
  const base = directory.base;
  const baseName = base?.name || (base?.kind === "empty_tree" ? "Empty tree" : "Base");
  return `<p>${esc(directory.branch || "Detached HEAD")} · Base ${esc(baseName)} <code>${esc(base?.oid || "")}</code> → Head <code>${esc(directory.head || "")}</code></p>`;
}

/** Only the user's opinion establishes their comparison baseline. */
export function reviewBaseline(review, rows = []) {
  const last = rows.filter((row) => (row.actor || row.author)?.kind === "user" && row.opinion?.snapshot_id).at(-1);
  return review?.snapshots.find((row) => row.id === last?.opinion.snapshot_id) || null;
}

const railButton = (mode, label, selection) => `<button class="task-review-range" type="button" data-review-range="${mode}" aria-pressed="${selection.mode === mode}">${label}</button>`;
function commitWhen(commit) {
  const stamp = commit.time || commit.authored_at || commit.author_time || commit.timestamp;
  const date = new Date(typeof stamp === "number" ? stamp * 1000 : stamp);
  return Number.isNaN(date.getTime()) ? "" : `· ${esc(ageText(date.toISOString()))}`;
}
const commitAuthor = (commit) => (typeof commit.author === "string" ? commit.author : commit.author?.name) || commit.author_name || "";
const commitAdditions = (commit) => Number(commit.additions ?? commit.insertions ?? 0);
function commitRailHtml(commit, selection) {
  const oid = commit.oid || commit.sha;
  return `<button type="button" class="task-review-commit" data-review-commit="${esc(oid)}" aria-pressed="${selection.commit === oid}"><strong>${esc(commit.subject)}</strong><span>${esc(commitAuthor(commit))} ${commitWhen(commit)}</span><span><code>${esc(oid.slice(0, 7))}</code><span class="task-review-count">+${commitAdditions(commit)} −${Number(commit.deletions ?? 0)}</span></span></button>`;
}
function railCommitsHtml(support, commits, selection) {
  if (!support.commits || !support.commit) return "";
  const entries = (commits?.commits || []).map((row) => commitRailHtml(row, selection)).join("");
  const more = commits?.next_cursor ? '<button class="btn" type="button" data-review-more-commits>Load more commits</button>' : "";
  return entries + more;
}
export function reviewRailHtml({ support = {}, selection = {}, commits = null, baseline = null }) {
  const since = support.since && baseline ? railButton("since", "Since my review", selection) : "";
  const error = commits?.read_error ? `<p class="sub">${esc(commits.read_error)}</p>` : "";
  return `<h2>Commits</h2>${railButton("all", "All changes", selection)}${since}${railCommitsHtml(support, commits, selection)}${error}`;
}

export function reviewComparisonBaseline(review, rows, selected) {
  const baseline = reviewBaseline(review, rows);
  return baseline && selected && baseline.number < selected.number ? baseline : null;
}

export function reviewSinceCount(record, baseline, directory) {
  if (!baseline || !directory) return null;
  const observation = record?.sync?.find((row) => row.directory_id === directory.id
    && row.snapshot_head === directory.head && row.reviewed_snapshot_id === baseline.id);
  return Number.isSafeInteger(observation?.commits_since_review) ? observation.commits_since_review : null;
}
