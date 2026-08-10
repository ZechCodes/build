// The project sidebar: pure model + HTML builders (no DOM, unit-testable).
//
// Each project is one block: its name, the branch its checkout has out with that
// branch's git status, the rail entries, and a Worktrees row holding whatever the
// entries did not show. WHICH entries appear and in what order is rail.js — this
// module is how they look.

import { esc } from "./text.js";
import { defaultRunTab } from "./taskActions.js";
import { RUN_STATE_LABEL, PLAN_STATE_LABEL } from "./entityPresentation.js";
import { dotState, railEntries, railWorktrees, runEntry } from "./rail.js";

export function buildSidebarModel({
  projects,
  runs,
  plans,
  externalWorktrees,
  primaryChanges,
  readIds,
  nowMs,
  pendingDone = new Set(),
}) {
  const visible = (kind, id) => !pendingDone.has(`${kind}:${id}`);
  return (projects || []).map((p) => {
    const myRuns = (runs || []).filter(
      (r) => r.project_id === p.project_id && visible("run", r.run_id),
    );
    const myPlans = (plans || []).filter(
      (pl) => pl.project_id === p.project_id && visible("plan", pl.plan_id),
    );
    const worktrees = (externalWorktrees || []).filter(
      (w) => w.project_id === p.project_id && visible("worktree", w.worktree_id),
    );
    const pc = (primaryChanges || []).find((c) => c.project_id === p.project_id) || null;
    // The run that owns the primary checkout is the project's main row, never
    // one more entry — the checkout line carries its dot. Only lifted when that
    // line exists to carry it; without a summary the run stays listed.
    const primaryRun = pc ? myRuns.find((r) => r.primary) : null;
    const entries = railEntries({
      runs: primaryRun ? myRuns.filter((r) => r !== primaryRun) : myRuns,
      plans: myPlans,
      worktrees,
      projectId: p.project_id,
      nowMs,
    });
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
            upstream: pc.upstream || null,
            comparison_ref: pc.comparison_ref || null,
            files_changed: pc.files_changed || 0,
            insertions: pc.insertions || 0,
            deletions: pc.deletions || 0,
            run: primaryRun ? runEntry(primaryRun) : null,
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

/** A row's floating git status — the three things worth knowing about a
 *  checkout at a glance, each shown only when it has something to say:
 *
 *    ↑n   commits ahead of the selected comparison ref
 *    ↓n   work it has not caught up with (out of date)
 *    +n −n  work that is not even committed
 */
function entryStatusHtml(status) {
  if (!status) return "";
  const parts = [];
  if (status.ahead) parts.push(`<span class="ssync">↑${status.ahead}</span>`);
  if (status.behind) parts.push(`<span class="ssync">↓${status.behind}</span>`);
  if (status.insertions) parts.push(`<em class="add">+${status.insertions}</em>`);
  if (status.deletions) parts.push(`<em class="del">-${status.deletions}</em>`);
  if (!parts.length) return "";
  return `<span class="sstat mono"${statusTitle(status)}>${parts.join(" ")}</span>`;
}

/** The ` title="…"` attribute spelling out each part of the status, or "" when
 *  there is nothing to spell out. Ref names come from the repo, so they are
 *  escaped like every other git-derived string. */
function statusTitle(status) {
  const said = [];
  const comparisonRef = status.comparisonRef || "the comparison branch";
  if (status.ahead) said.push(`↑${status.ahead} ahead of ${comparisonRef}`);
  if (status.behind) said.push(`↓${status.behind} behind ${comparisonRef}`);
  const changes = [status.insertions ? `+${status.insertions}` : "", status.deletions ? `-${status.deletions}` : ""]
    .filter(Boolean)
    .join(" ");
  // What the +/− counts is not the same everywhere: a checkout's is what sits
  // in it uncommitted, a task's is the whole delta it exists to produce.
  if (changes) said.push(`${changes} ${status.changesLabel || "changed"}`);
  return said.length ? ` title="${esc(said.join(" · "))}"` : "";
}

/** The label under a row's name: what a run/issue is, when it has no git status
 *  of its own to show. */
const STATE_LABELS = { ...RUN_STATE_LABEL, ...PLAN_STATE_LABEL };

/** The flat rail's second line: which project this row lives in, and the git
 *  status that would otherwise have to fight the name for the first one.
 *
 *  A row under a project block carries three things on one line and the heading
 *  answers the fourth. Without the heading the row has to say the project too,
 *  and four things do not fit a 272px rail — the name is what loses, which is
 *  the one thing you are scanning for. So the name keeps the first line, and
 *  the project and the status share the second. */
function metaLine(entry, statusHtml) {
  const tag = entry.project_name ? `<span class="sproj-tag mono">${esc(entry.project_name)}</span>` : "";
  return tag || statusHtml ? `<span class="smeta">${tag}${statusHtml}</span>` : "";
}

/** One rail row: `[dot] name [git status]`. The name falls back to the branch —
 *  a worktree with nothing filed behind it is still its branch. A main row is
 *  its project's checkout: named by the branch it has out, and not something you
 *  finish, so it carries no Done control.
 *  `opts`: { showProject } — label the row with its project (the flat rail). */
function entryRow(entry, ui, { showProject = false } = {}) {
  const activeId =
    entry.kind === "run"
      ? ui.activeRunId
      : entry.kind === "plan"
        ? ui.activePlanId
        : entry.kind === "main"
          ? ui.activeMainProjectId
          : ui.activeWorktreeId;
  const label = entry.name || entry.branch || "(untitled)";
  const right = entry.status
    ? entryStatusHtml(entry.status)
    : `<span class="sstate">${esc(STATE_LABELS[entry.state] || entry.state || "")}</span>`;
  const data =
    entry.kind === "run"
      ? `data-run="${esc(entry.id)}" data-tab="${defaultRunTab({ state: entry.state })}"`
      : entry.kind === "plan"
        ? `data-plan="${esc(entry.id)}"`
        : entry.kind === "main"
          ? `data-main="${esc(entry.id)}"`
          : `data-wt="${esc(entry.id)}"`;
  const done =
    entry.kind === "plan" && entry.can_archive
      ? `<button class="btn mini" data-done-plan="${esc(entry.id)}" type="button" aria-label="Archive plan ${esc(label)}">Done</button><span class="warn" data-done-error hidden></span>`
      : entry.kind === "run" && entry.can_finish
        ? `<button class="btn mini" data-done-run="${esc(entry.id)}" type="button" aria-label="Finish task ${esc(label)}">Done</button><span class="warn" data-done-error hidden></span>`
        : entry.kind === "worktree" && entry.can_finish
          ? `<button class="btn mini" data-done-worktree="${esc(entry.id)}" type="button" aria-label="Finish worktree ${esc(label)}">Done</button><span class="warn" data-done-error hidden></span>`
          : "";
  const title = entry.kind === "main" && entry.path ? `${label} — ${entry.path}` : label;
  const name = `<span class="stitle${entry.kind === "main" ? " mono" : ""}">${esc(label)}</span>`;
  return `<div class="srow sentry ${entry.kind === "main" ? "smain-row " : ""}${entry.id === activeId ? "active" : ""}" ${data} data-project="${esc(entry.project_id)}" title="${esc(title)}">
    ${dotHtml(entry)}${name}${showProject ? "" : right}${done}${showProject ? metaLine(entry, right) : ""}</div>`;
}

/** The primary checkout's git status, using its upstream when tracked and the
 *  local base branch otherwise, plus its uncommitted working-tree delta. */
export function checkoutStatusHtml(primary) {
  return entryStatusHtml({
    ahead: primary.ahead,
    behind: primary.behind,
    comparisonRef: primary.comparison_ref || primary.upstream || null,
    insertions: primary.insertions,
    deletions: primary.deletions,
    changesLabel: "uncommitted",
  });
}

// The project's primary checkout, as the second line of its header: the branch
// it has out and that branch's git status. No glyph and no label word — its
// place under the project name says which checkout it is. When a run owns the
// checkout, this line is that run's row, so it carries the run's dot.
function checkoutLine(m, ui) {
  if (!m.primary) return "";
  const active = ui.activeMainProjectId === m.project_id ? "active" : "";
  const title = esc(m.primary.branch) + (m.primary.path ? ` — ${esc(m.primary.path)}` : "");
  return `<div class="srow smain-line ${active}" data-main="${esc(m.project_id)}" title="${title}">
    ${m.primary.run ? dotHtml(m.primary.run) : ""}<span class="stitle mono">${esc(m.primary.branch)}</span>${checkoutStatusHtml(m.primary)}</div>`;
}

/** The `Worktrees ›` row: everything the entries above did not already show —
 *  worktrees you made outside Build, and ones that have fallen out of the recent
 *  list. A place to look something up, so it is ordered by name.
 *  `key` is what the fold toggles under: a project id per block, or `__all` for
 *  the one fold the flat rail has. */
function worktreeLine(worktrees, key, open, ui, { showProject = false } = {}) {
  if (!worktrees.length) return "";
  const list = open
    ? `<div class="swt-list">${worktrees
        .map(
          (w) => {
            const label = w.name || w.branch || "(detached)";
            const done = w.can_finish
              ? `<button class="btn mini" data-done-worktree="${esc(w.id)}" type="button" aria-label="Finish worktree ${esc(label)}">Done</button><span class="warn" data-done-error hidden></span>`
              : "";
            const status = entryStatusHtml(w.status);
            return `<div class="srow swt-item ${w.id === ui.activeWorktreeId ? "active" : ""}" data-wt="${esc(w.id)}" data-project="${esc(w.project_id)}" title="${esc(w.branch || w.name || "")}">
      <span class="stitle mono">${esc(label)}</span>${showProject ? "" : status}${done}${showProject ? metaLine(w, status) : ""}</div>`;
          },
        )
        .join("")}</div>`
    : "";
  const activeInside = worktrees.some((w) => w.id === ui.activeWorktreeId);
  return `<div class="srow swt-line ${activeInside && !open ? "active" : ""}" data-wtline="${esc(key)}">
    <span class="stitle dim">Worktrees <span class="n">${worktrees.length}</span></span>
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
    ${worktreeLine(m.worktrees, m.project_id, ui.wtOpen.has(m.project_id), ui)}</div></div>`;
}

/** The key the flat rail's single Worktrees fold toggles under — no project owns
 *  it, so it cannot collide with a project id. */
export const ALL_WORKTREES_KEY = "__all";

/** The header both rails share: which rail you are on, and the add button.
 *  `ui.mode` is "projects" or "all" (core/railMode.js). */
function railHeadHtml(ui) {
  const mode = ui.mode === "all" ? "all" : "projects";
  const button = (value, label) =>
    `<button class="railmode ${value === mode ? "active" : ""}" data-railmode="${value}" type="button" aria-pressed="${value === mode}">${label}</button>`;
  return `<div class="side-head"><span class="railmodes">${button("projects", "Projects")}${button("all", "All")}</span>
    <span class="side-actions">
      <button class="iconbtn" id="side-add" title="Add a project">+</button>
    </span></div>`;
}

export function sidebarHtml(model, ui) {
  const projects = model.map((m) => projectHtml(m, ui)).join("");
  return `${railHeadHtml(ui)}
  <div class="side-projects">${projects || '<div class="dim sempty">No projects yet — add one.</div>'}</div>`;
}

/** The flat rail: every project's work in one list, each row saying where it
 *  lives, then the one Worktrees fold holding what the list did not show.
 *  `model` is core/allRail.js's `{entries, worktrees, unread}`. */
export function allRailHtml(model, ui) {
  const entries = model.entries.map((entry) => entryRow(entry, ui, { showProject: true })).join("");
  return `${railHeadHtml(ui)}
  <div class="side-all">
    ${entries || '<div class="sempty dim">Nothing running.</div>'}
    ${worktreeLine(model.worktrees, ALL_WORKTREES_KEY, ui.wtOpen.has(ALL_WORKTREES_KEY), ui, { showProject: true })}</div>`;
}
