// The project sidebar: pure model + HTML builders (no DOM, unit-testable).
//
// Each project is one block: its name, the branch its checkout has out with that
// branch's git status, the rail entries, and a Worktrees row holding whatever the
// entries did not show. WHICH entries appear and in what order is rail.js — this
// module is how they look.

import { esc } from "./text.js";
import { defaultRunTab } from "./taskActions.js";
import { RUN_STATE_LABEL, PLAN_STATE_LABEL } from "./entityPresentation.js";
import { dotState, railEntries, railWorktrees } from "./rail.js";

export function buildSidebarModel({ projects, runs, plans, externalWorktrees, primaryChanges, readIds, nowMs }) {
  return (projects || []).map((p) => {
    const myRuns = (runs || []).filter((r) => r.project_id === p.project_id);
    const myPlans = (plans || []).filter((pl) => pl.project_id === p.project_id);
    const worktrees = (externalWorktrees || []).filter((w) => w.project_id === p.project_id);
    const entries = railEntries({
      runs: myRuns,
      plans: myPlans,
      worktrees,
      projectId: p.project_id,
      nowMs,
    });
    const pc = (primaryChanges || []).find((c) => c.project_id === p.project_id) || null;
    // The badge counts what is waiting on you and unread — the same question the
    // yellow dot answers, so the two can never disagree.
    const unread =
      myRuns.filter((r) => r.needs_attention && !readIds.has(r.run_id)).length +
      myPlans.filter((pl) => pl.needs_attention && !readIds.has(pl.plan_id)).length;
    return {
      project_id: p.project_id,
      name: p.name,
      unread,
      entries,
      worktrees: railWorktrees({ worktrees, entries, projectId: p.project_id }),
      primary: pc
        ? {
            branch: pc.branch,
            path: pc.path || null,
            ahead: Number.isFinite(pc.ahead) ? pc.ahead : null,
            behind: Number.isFinite(pc.behind) ? pc.behind : null,
            files_changed: pc.files_changed || 0,
            insertions: pc.insertions || 0,
            deletions: pc.deletions || 0,
          }
        : null,
    };
  });
}

/** The dot: a pulse while an agent works, yellow when something finished and you
 *  have not looked since, grey once seen. Three states, one glance. */
function dotHtml(entry) {
  const state = dotState(entry);
  return `<span class="sdot sdot-${state}" title="${
    state === "working" ? "an agent is working" : state === "unseen" ? "finished — not seen yet" : "seen"
  }"></span>`;
}

/** A row's floating git status: how far the branch has diverged, then what the
 *  working tree holds uncommitted. Each part shows only when it has something to
 *  say. The divergence is measured against the branch's upstream when it tracks
 *  one and the base branch otherwise, so — since the same ↑2 means a different
 *  thing either way — the row names which on hover. */
function entryStatusHtml(status) {
  if (!status) return "";
  const parts = [];
  if (status.ahead) parts.push(`<span class="ssync">↑${status.ahead}</span>`);
  if (status.behind) parts.push(`<span class="ssync">↓${status.behind}</span>`);
  if (status.insertions) parts.push(`<em class="add">+${status.insertions}</em>`);
  if (status.deletions) parts.push(`<em class="del">-${status.deletions}</em>`);
  if (!parts.length) return "";
  return `<span class="sstat mono"${syncTitle(status)}>${parts.join(" ")}</span>`;
}

/** The ` title="…"` attribute naming what ahead/behind were measured against,
 *  or "" when nothing diverged or the bridge did not say. The ref name comes
 *  from the repo, so it is escaped like every other git-derived string. */
function syncTitle(status) {
  if (!status.syncBase || (!status.ahead && !status.behind)) return "";
  const sides = [status.ahead ? `↑${status.ahead} ahead` : "", status.behind ? `↓${status.behind} behind` : ""]
    .filter(Boolean)
    .join(", ");
  return ` title="${esc(sides)} of ${esc(status.syncBase)}"`;
}

/** The label under a row's name: what a run/issue is, when it has no git status
 *  of its own to show. */
const STATE_LABELS = { ...RUN_STATE_LABEL, ...PLAN_STATE_LABEL };

/** One rail row: `[dot] name [git status]`. The name falls back to the branch —
 *  a worktree with nothing filed behind it is still its branch. */
function entryRow(entry, ui) {
  const activeId =
    entry.kind === "run" ? ui.activeRunId : entry.kind === "plan" ? ui.activePlanId : ui.activeWorktreeId;
  const label = entry.name || entry.branch || "(untitled)";
  const right = entry.status
    ? entryStatusHtml(entry.status)
    : `<span class="sstate">${esc(STATE_LABELS[entry.state] || entry.state || "")}</span>`;
  const data =
    entry.kind === "run"
      ? `data-run="${esc(entry.id)}" data-tab="${defaultRunTab({ state: entry.state })}"`
      : entry.kind === "plan"
        ? `data-plan="${esc(entry.id)}"`
        : `data-wt="${esc(entry.id)}"`;
  return `<div class="srow sentry ${entry.id === activeId ? "active" : ""}" ${data} data-project="${esc(entry.project_id)}" title="${esc(label)}">
    ${dotHtml(entry)}<span class="stitle">${esc(label)}</span>${right}</div>`;
}

/** The checkout's git status, in the order a `git status` would tell it: where
 *  the branch sits against its upstream, then what the working tree holds. Each
 *  part appears only when it has something to say. */
export function checkoutStatusHtml(primary) {
  const parts = [];
  if (primary.ahead) parts.push(`<span class="ssync">↑${primary.ahead}</span>`);
  if (primary.behind) parts.push(`<span class="ssync">↓${primary.behind}</span>`);
  if (primary.insertions) parts.push(`<em class="add">+${primary.insertions}</em>`);
  if (primary.deletions) parts.push(`<em class="del">-${primary.deletions}</em>`);
  return parts.length ? `<span class="sstat mono">${parts.join(" ")}</span>` : "";
}

// The project's primary checkout, as the second line of its header: the branch
// it has out and that branch's git status. No glyph and no label word — its
// place under the project name says which checkout it is.
function checkoutLine(m, ui) {
  if (!m.primary) return "";
  const active = ui.activeMainProjectId === m.project_id ? "active" : "";
  const title = esc(m.primary.branch) + (m.primary.path ? ` — ${esc(m.primary.path)}` : "");
  return `<div class="srow smain-line ${active}" data-main="${esc(m.project_id)}" title="${title}">
    <span class="stitle mono">${esc(m.primary.branch)}</span>${checkoutStatusHtml(m.primary)}</div>`;
}

/** The `Worktrees ›` row: everything the entries above did not already show —
 *  worktrees you made outside Build, and ones that have fallen out of the recent
 *  list. A place to look something up, so it is ordered by name. */
function worktreeLine(m, open, ui) {
  if (!m.worktrees.length) return "";
  const list = open
    ? `<div class="swt-list">${m.worktrees
        .map(
          (w) => `<div class="srow swt-item ${w.id === ui.activeWorktreeId ? "active" : ""}" data-wt="${esc(w.id)}" data-project="${esc(w.project_id)}" title="${esc(w.branch || w.name || "")}">
      <span class="stitle mono">${esc(w.name || w.branch || "(detached)")}</span>${entryStatusHtml(w.status)}</div>`,
        )
        .join("")}</div>`
    : "";
  const activeInside = m.worktrees.some((w) => w.id === ui.activeWorktreeId);
  return `<div class="srow swt-line ${activeInside && !open ? "active" : ""}" data-wtline="${esc(m.project_id)}">
    <span class="stitle dim">Worktrees <span class="n">${m.worktrees.length}</span></span>
    <span class="swt-caret">${open ? "▾" : "›"}</span></div>${list}`;
}

/** One project's block: name, its checkout's branch + status, the rail entries,
 *  then the Worktrees row holding everything not shown above.
 *  `ui`: { closed:Set, wtOpen:Set, activeRunId, activePlanId, activeWorktreeId,
 *  activeProjectId, activeMainProjectId } */
export function projectHtml(m, ui) {
  const open = !ui.closed.has(m.project_id);
  const badge = m.unread ? `<span class="badge sbadge">${m.unread}</span>` : "";
  // Two lines that survive collapsing: a shut project still says which branch it
  // has out and whether that tree is dirty. The chevron toggles, the name opens
  // the project, the checkout line opens its Changes tab.
  const head = `<div class="sproj-head ${ui.activeProjectId === m.project_id ? "active" : ""}">
    <button class="chevbtn" data-chev="${esc(m.project_id)}" title="${open ? "Collapse" : "Expand"}">${open ? "▾" : "▸"}</button>
    <span class="sproj-open" data-open="${esc(m.project_id)}" title="Open project"><span class="sproj-name mono">${esc(m.name)}</span></span>
    ${badge}</div>${checkoutLine(m, ui)}`;
  const block = `sproj ${ui.activeProjectId === m.project_id ? "active" : ""}`.trim();
  if (!open) return `<div class="${block}">${head}</div>`;

  const entries = m.entries.map((entry) => entryRow(entry, ui)).join("");
  return `<div class="${block}">${head}<div class="sproj-body">
    ${entries || '<div class="sempty dim">Nothing running.</div>'}
    ${worktreeLine(m, ui.wtOpen.has(m.project_id), ui)}</div></div>`;
}

export function sidebarHtml(model, ui) {
  const projects = model.map((m) => projectHtml(m, ui)).join("");
  return `<div class="side-head"><span class="sseclabel">Projects</span>
    <span class="side-actions">
      <button class="iconbtn" id="side-add" title="Add a project">+</button>
    </span></div>
  <div class="side-projects">${projects || '<div class="dim sempty">No projects yet — add one.</div>'}</div>`;
}
