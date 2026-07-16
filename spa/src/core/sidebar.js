// The project sidebar: pure model + HTML builders (no DOM, unit-testable).
// Every page shows this rail — projects with unread badges, what needs you,
// what's running, what just finished, and the project's worktrees.

import { esc, humanAge } from "./text.js";

/** Truncation is CSS's job (ellipsis); classification is ours. */
const TERMINAL = new Set(["merged", "abandoned"]);
const DONE_RECENTLY_CAP = 3;
const DONE_RECENTLY_WINDOW_MS = 7 * 24 * 3600 * 1000;

function ageSeconds(iso, nowMs) {
  const t = Date.parse(iso || "");
  return Number.isFinite(t) ? Math.max(0, (nowMs - t) / 1000) : null;
}

/** Group the task feed into the per-project sidebar model. `primaryChanges` is
 *  task.list's cached primary-checkout summary, attached per project as
 *  `m.primary = {branch, files_changed, insertions, deletions} | null`. */
export function buildSidebarModel({ projects, tasks, externalWorktrees, primaryChanges, readIds, nowMs }) {
  return (projects || []).map((p) => {
    const mine = (tasks || []).filter((t) => t.project_id === p.project_id);
    const needsYou = mine.filter((t) => t.needs_attention && !TERMINAL.has(t.state));
    const running = mine.filter((t) => !t.needs_attention && !TERMINAL.has(t.state));
    const doneRecently = mine
      .filter((t) => TERMINAL.has(t.state))
      .map((t) => ({ ...t, age_s: ageSeconds(t.updated_at, nowMs) }))
      .filter((t) => t.age_s !== null && t.age_s * 1000 <= DONE_RECENTLY_WINDOW_MS)
      .sort((a, b) => a.age_s - b.age_s)
      .slice(0, DONE_RECENTLY_CAP);
    const worktrees = (externalWorktrees || []).filter((w) => w.project_id === p.project_id);
    const pc = (primaryChanges || []).find((c) => c.project_id === p.project_id) || null;
    return {
      project_id: p.project_id,
      name: p.name,
      unread: needsYou.filter((t) => !readIds.has(t.task_id)).length,
      needsYou,
      running,
      doneRecently,
      worktrees,
      uncommitted: worktrees.filter((w) => (w.dirty_files || 0) > 0).length,
      primary: pc
        ? { branch: pc.branch, files_changed: pc.files_changed || 0, insertions: pc.insertions || 0, deletions: pc.deletions || 0 }
        : null,
    };
  });
}

const statHtml = (stat) =>
  stat
    ? `<span class="sstat mono"><em class="add">+${stat.insertions}</em> <em class="del">-${stat.deletions}</em></span>`
    : "";

function taskRow(t, { icon, right, cls = "" }) {
  const tab = t.state === "plan_review" ? "plan" : "diff";
  return `<div class="srow ${cls}" data-task="${esc(t.task_id)}" data-tab="${tab}">
    <span class="sicon">${icon}</span><span class="stitle">${esc(t.goal)}</span>${right}</div>`;
}

const doneIcon = (t) => (t.state === "merged" ? "✓" : "×");

function section(label, rowsHtml) {
  return rowsHtml ? `<div class="ssec"><div class="sseclabel">${label}</div>${rowsHtml}</div>` : "";
}

// The primary-checkout "main" row: branch + a dirty count when the working tree
// has uncommitted changes. Links to #/main/<projectId> (wired by the view).
function mainLine(m) {
  if (!m.primary) return "";
  const dirty = m.primary.files_changed
    ? ` <span class="swt-dirty">· ${m.primary.files_changed} uncommitted</span>`
    : "";
  return `<div class="srow smain-line" data-main="${esc(m.project_id)}">
    <span class="sicon">⌂</span><span class="stitle mono">main <span class="dim">${esc(m.primary.branch)}</span>${dirty}</span></div>`;
}

function worktreeLine(m, open) {
  if (!m.worktrees.length) return "";
  const label = `${m.worktrees.length} worktree${m.worktrees.length === 1 ? "" : "s"}`;
  const dirty = m.uncommitted ? ` <span class="swt-dirty">· ${m.uncommitted} uncommitted</span>` : "";
  const list = open
    ? `<div class="swt-list">${m.worktrees
        .map(
          (w) => `<div class="srow swt-item" data-wt="${esc(w.worktree_id)}" data-project="${esc(w.project_id)}">
      <span class="sicon">⌥</span><span class="stitle mono">${esc(w.branch || "(detached)")}</span>${
        (w.dirty_files || 0) > 0 ? '<span class="swt-dirty">●</span>' : ""
      }</div>`
        )
        .join("")}</div>`
    : "";
  return `<div class="srow swt-line" data-wtline="${esc(m.project_id)}">
    <span class="sicon">⌥</span><span class="stitle dim">${label}${dirty}</span></div>${list}`;
}

/** An outline folder, sized for the rail (the mock's project glyph). */
const FOLDER_ICON = `<svg class="sfolder" width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" aria-hidden="true"><path d="M1.75 4.25a1 1 0 0 1 1-1h3.1l1.5 1.75h6.9a1 1 0 0 1 1 1v6.75a1 1 0 0 1-1 1H2.75a1 1 0 0 1-1-1V4.25z"/></svg>`;

/** One project's block. `ui`: { closed:Set, wtOpen:Set, activeTaskId, activeProjectId } */
export function projectHtml(m, ui) {
  const open = !ui.closed.has(m.project_id);
  const badge = m.unread ? `<span class="badge sbadge">${m.unread}</span>` : "";
  // Two distinct targets: the chevron expands/collapses; the name opens the
  // project page (and the wiring expands the project as it navigates).
  const head = `<div class="sproj-head ${ui.activeProjectId === m.project_id ? "active" : ""}">
    <button class="chevbtn" data-chev="${esc(m.project_id)}" title="${open ? "Collapse" : "Expand"}">${open ? "▾" : "▸"}</button>
    <span class="sproj-open" data-open="${esc(m.project_id)}" title="Open project">${FOLDER_ICON}<span class="sproj-name mono">${esc(m.name)}</span></span>
    ${badge}</div>`;
  if (!open) return `<div class="sproj">${head}</div>`;

  const active = (t) => (t.task_id === ui.activeTaskId ? "active" : "");
  const needs = m.needsYou
    .map((t) =>
      taskRow(t, {
        icon: t.state === "blocked" || t.state === "failed" ? '<span class="warn">▲</span>' : "✦",
        right: statHtml(t.stat),
        cls: `attn ${active(t)}`,
      })
    )
    .join("");
  const running = m.running
    .map((t) =>
      taskRow(t, {
        icon: "●",
        right:
          (t.last_error ? '<span class="warn">▲</span> ' : "") +
          (t.stat && t.stat.files_changed ? statHtml(t.stat) : `<span class="sstate">${esc(t.state)}</span>`),
        cls: `work ${active(t)}`,
      })
    )
    .join("");
  const done = m.doneRecently
    .map((t) =>
      taskRow(t, {
        icon: doneIcon(t),
        right: `<span class="sage">${humanAge(t.age_s)}</span>`,
        cls: "done",
      })
    )
    .join("");
  return `<div class="sproj">${head}<div class="sproj-body">
    ${section("Needs you", needs)}${section("Running", running)}${section("Done recently", done)}
    ${mainLine(m)}${worktreeLine(m, ui.wtOpen.has(m.project_id))}</div></div>`;
}

/** The whole rail. */
export function sidebarHtml(model, ui) {
  const projects = model.map((m) => projectHtml(m, ui)).join("");
  return `<div class="side-head"><span class="sseclabel">Projects</span>
    <span class="side-actions">
      <button class="iconbtn" id="side-add" title="Add a project">+</button>
    </span></div>
  <div class="side-projects">${projects || '<div class="dim sempty">No projects yet — add one.</div>'}</div>`;
}
