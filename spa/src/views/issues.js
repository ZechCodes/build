// The project's Issues tab: every issue filed against this project, open ones
// first, in one list.
//
// An issue IS the plan record — filing one states what you want and Build drafts
// the plan to implement it — so this reads plans off board.list and calls them
// what the product calls them. Closed means abandoned, or merged by the run that
// implemented it; everything else is open, which is what you came here to see.

import { esc, humanAge } from "../core/text.js";
import { PLAN_STATE_LABEL, planChipClass, planStateWord } from "./shared.js";


const CLOSED_STATES = new Set(["abandoned"]);

const ageSeconds = (iso, nowMs) => {
  const t = Date.parse(iso || "");
  return Number.isFinite(t) ? Math.max(0, (nowMs - t) / 1000) : null;
};

/** Pure: split a project's issues into open and closed, each newest-first. */
export function bucketIssues(plans, projectId, nowMs = Date.now()) {
  const mine = (plans || [])
    .filter((p) => p.project_id === projectId)
    .map((p) => ({ ...p, age_s: ageSeconds(p.updated_at, nowMs) }));
  const byRecency = (a, b) => (a.age_s ?? Infinity) - (b.age_s ?? Infinity);
  return {
    open: mine.filter((p) => !CLOSED_STATES.has(p.state)).sort(byRecency),
    closed: mine.filter((p) => CLOSED_STATES.has(p.state)).sort(byRecency),
  };
}

const issueRow = (p) => {
  const openComments = (p.stages || []).reduce((n, s) => n + (s.open_comments || 0), 0);
  const meta = [
    p.active_run_id ? "implementing" : planStateWord(p.state),
    openComments ? `${openComments} 💬` : null,
    p.age_s === null || p.age_s === undefined ? null : humanAge(p.age_s),
  ].filter(Boolean);
  return `<div class="issue-row" data-plan="${esc(p.plan_id)}" title="${esc(p.goal || "")}">
    <span class="issue-title">${esc(p.goal || "")}</span>
    <span class="chip ${planChipClass(p.state)}">${esc(PLAN_STATE_LABEL[p.state] || p.state || "")}</span>
    <span class="issue-meta mono">${meta.map((m) => esc(m)).join(" · ")}</span>
  </div>`;
};

const section = (label, rows) =>
  rows.length ? `<div class="issue-section"><h2>${label} <span class="n">${rows.length}</span></h2>${rows.map(issueRow).join("")}</div>` : "";

/** Pure: the tab body for a project's issues. The empty state offers the verb
 *  itself rather than pointing at where the verb lives — a UI that has to give
 *  directions to its own buttons has already lost. */
export function issuesHtml({ open, closed }) {
  if (!open.length && !closed.length) {
    return `<div class="issues issues-empty">
      <div class="emptystate">
        <div class="emptystate-title">No issues yet</div>
        <div class="emptystate-sub">Say what you want done and Build drafts the plan to do it.</div>
        <button class="btn" type="button" data-newissue="1">New issue</button>
      </div></div>`;
  }
  return `<div class="issues">${section("OPEN", open)}${section("CLOSED", closed)}</div>`;
}

/**
 * Mount the Issues tab. Polls board.list on the inbox's cadence and routes a
 * click to that issue's surface. Returns { dispose() }.
 */
export function mountIssuesTab(host, { projectId, callRpc, navigate, onNewIssue, pollMs = 2500 }) {
  let disposed = false;
  const draw = (plans) => {
    host.innerHTML = issuesHtml(bucketIssues(plans, projectId));
    host.querySelectorAll(".issue-row[data-plan]").forEach((row) => {
      row.onclick = () => navigate({ name: "plan", projectId, id: row.dataset.plan, tab: "review" });
    });
    const newIssue = host.querySelector("[data-newissue]");
    if (newIssue && onNewIssue) newIssue.onclick = () => onNewIssue();
  };
  const load = async () => {
    try {
      const board = await callRpc("board.list");
      if (!disposed) draw(board.plans || []);
    } catch {
      /* offline / transient — the poll retries */
    }
  };
  load();
  const poll = setInterval(load, pollMs);
  return {
    dispose() {
      disposed = true;
      clearInterval(poll);
    },
  };
}
