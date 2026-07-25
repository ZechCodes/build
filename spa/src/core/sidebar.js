// The project sidebar: pure model + HTML builders (no DOM, unit-testable).
// Every page shows this rail — projects with unread badges, the plans authored
// in each project, what needs you, what's running, what just finished, and the
// project's worktrees. The plan/run split means each project carries both its
// plans (project-scoped) and its runs (worktree-scoped; "Tasks" in the UI).

import { esc, humanAge } from "./text.js";
import { defaultRunTab } from "./taskActions.js";
import { RUN_STATE_LABEL, PLAN_STATE_LABEL } from "./entityPresentation.js";

/** Truncation is CSS's job (ellipsis); classification is ours. */
const RUN_TERMINAL = new Set(["merged", "abandoned", "archived"]);
const PLAN_TERMINAL = new Set(["abandoned"]);
const DONE_RECENTLY_CAP = 3;
const DONE_RECENTLY_WINDOW_MS = 7 * 24 * 3600 * 1000;

function ageSeconds(iso, nowMs) {
  const t = Date.parse(iso || "");
  return Number.isFinite(t) ? Math.max(0, (nowMs - t) / 1000) : null;
}

// The recently-finished tail of a terminal collection: windowed to 7 days,
// newest first, capped. Shared by runs and plans (each supplies its own id).
function doneRecently(items, nowMs) {
  return items
    .map((x) => ({ ...x, age_s: ageSeconds(x.updated_at, nowMs) }))
    .filter((x) => x.age_s !== null && x.age_s * 1000 <= DONE_RECENTLY_WINDOW_MS)
    .sort((a, b) => a.age_s - b.age_s)
    .slice(0, DONE_RECENTLY_CAP);
}

/** Group the feed into the per-project sidebar model. Runs bucket as
 *  needs-you / running / done-recently; plans bucket as needs-you / drafting /
 *  approved / done. `primaryChanges` is board.list's cached primary-checkout
 *  summary, attached per project as `m.primary`. */
export function buildSidebarModel({ projects, runs, plans, externalWorktrees, primaryChanges, readIds, nowMs }) {
  return (projects || []).map((p) => {
    const myRuns = (runs || []).filter((r) => r.project_id === p.project_id);
    const needsYou = myRuns.filter((r) => r.needs_attention && !RUN_TERMINAL.has(r.state));
    const running = myRuns.filter((r) => !r.needs_attention && !RUN_TERMINAL.has(r.state));
    const runsDone = doneRecently(myRuns.filter((r) => RUN_TERMINAL.has(r.state)), nowMs);

    const myPlans = (plans || []).filter((pl) => pl.project_id === p.project_id);
    const planNeedsYou = myPlans.filter((pl) => pl.needs_attention && !PLAN_TERMINAL.has(pl.state));
    const planApproved = myPlans.filter((pl) => pl.state === "approved");
    const planDrafting = myPlans.filter(
      (pl) => !pl.needs_attention && !PLAN_TERMINAL.has(pl.state) && pl.state !== "approved"
    );
    const plansDone = doneRecently(myPlans.filter((pl) => PLAN_TERMINAL.has(pl.state)), nowMs);

    const worktrees = (externalWorktrees || []).filter((w) => w.project_id === p.project_id);
    const pc = (primaryChanges || []).find((c) => c.project_id === p.project_id) || null;
    const unread =
      needsYou.filter((r) => !readIds.has(r.run_id)).length +
      planNeedsYou.filter((pl) => !readIds.has(pl.plan_id)).length;
    return {
      project_id: p.project_id,
      name: p.name,
      unread,
      needsYou,
      running,
      doneRecently: runsDone,
      planNeedsYou,
      planApproved,
      planDrafting,
      plansDone,
      worktrees,
      uncommitted: worktrees.filter((w) => (w.dirty_files || 0) > 0).length,
      primary: pc
        ? { branch: pc.branch, path: pc.path || null, files_changed: pc.files_changed || 0, insertions: pc.insertions || 0, deletions: pc.deletions || 0 }
        : null,
    };
  });
}

const statHtml = (stat) =>
  stat
    ? `<span class="sstat mono"><em class="add">+${stat.insertions}</em> <em class="del">-${stat.deletions}</em></span>`
    : "";

// A run row ("Tasks" in the UI), keyed by run_id, routing to the run surface's
// default tab — Stages for a run parked at the stage gate, Changes otherwise —
// so the sidebar opens the same tab notifications does (defaultRunTab).
function runRow(r, { icon, right, cls = "" }) {
  const title = `${esc(r.goal)} — ${esc(RUN_STATE_LABEL[r.state] || r.state || "")}`;
  return `<div class="srow ${cls}" data-run="${esc(r.run_id)}" data-project="${esc(r.project_id)}" data-tab="${defaultRunTab(r)}" title="${title}">
    <span class="sicon">${icon}</span><span class="stitle">${esc(r.goal)}</span>${right}</div>`;
}

const doneIcon = (r) => (r.state === "merged" ? "✓" : "×");

// The plan's short rail state word (like the run rows' state span). Terse
// lowercase so the rail stays scannable; the full label rides the row's title.
const PLAN_STATE_WORD = {
  plan_review: "review",
  drafting: "drafting",
  created: "drafting",
  approved: "ready",
  blocked: "blocked",
  failed: "failed",
  idle_unreported: "idle",
  interrupted: "interrupted",
  abandoned: "abandoned",
};
export const planStateWord = (state) => PLAN_STATE_WORD[state] || state || "";

// A plan row (project-scoped), keyed by plan_id, routing to the plan cockpit.
// Plans and runs share the rail, so a plan row carries its own glyph set and a
// distinct data attribute — never a data-run — keeping the two click paths apart.
// When the caller supplies no `right` (e.g. no open comments), the plan's state
// word fills that slot so a plan row is as legible as a run row.
function planRow(p, { icon, right = "", cls = "" }) {
  const title = `${esc(p.goal)} — ${esc(PLAN_STATE_LABEL[p.state] || p.state || "")}`;
  const rightHtml = right || `<span class="sstate">${esc(planStateWord(p.state))}</span>`;
  return `<div class="srow splan ${cls}" data-plan="${esc(p.plan_id)}" data-project="${esc(p.project_id)}" title="${title}">
    <span class="sicon">${icon}</span><span class="stitle">${esc(p.goal)}</span>${rightHtml}</div>`;
}

const openCommentsRight = (p) => {
  const n = (p.stages || []).reduce((sum, s) => sum + (s.open_comments || 0), 0);
  return n ? `<span class="sstate">${n} 💬</span>` : "";
};

function section(label, rowsHtml) {
  return rowsHtml ? `<div class="ssec"><div class="sseclabel">${label}</div>${rowsHtml}</div>` : "";
}

// The project's primary checkout, as the second line of its header: the branch
// it has out, plus a dirty count when the working tree has uncommitted changes.
// Links to #/project/<projectId>/changes.
//
// It carries NO label word. This row used to read "main <branch>", where "main"
// meant "the checkout, not a worktree" — but next to a branch called feat/…, it
// read as a branch name that disagreed with the one beside it. The ⌂ glyph makes
// the same distinction against the worktree rows' fork glyph, silently.
function checkoutLine(m, ui) {
  if (!m.primary) return "";
  const dirty = m.primary.files_changed
    ? ` <span class="swt-dirty">· ${m.primary.files_changed} uncommitted</span>`
    : "";
  const active = ui.activeMainProjectId === m.project_id ? "active" : "";
  const title = esc(m.primary.branch) + (m.primary.path ? ` — ${esc(m.primary.path)}` : "");
  return `<div class="srow smain-line ${active}" data-main="${esc(m.project_id)}" title="${title}">
    <span class="sicon">⌂</span><span class="stitle mono">${esc(m.primary.branch)}${dirty}</span></div>`;
}

function worktreeLine(m, open, ui) {
  if (!m.worktrees.length) return "";
  const label = `${m.worktrees.length} worktree${m.worktrees.length === 1 ? "" : "s"}`;
  const dirty = m.uncommitted ? ` <span class="swt-dirty">· ${m.uncommitted} uncommitted</span>` : "";
  const activeInProject = m.worktrees.some((w) => w.worktree_id === ui.activeWorktreeId);
  const list = open
    ? `<div class="swt-list">${m.worktrees
        .map(
          (w) => `<div class="srow swt-item ${w.worktree_id === ui.activeWorktreeId ? "active" : ""}" data-wt="${esc(w.worktree_id)}" data-project="${esc(w.project_id)}" title="${esc(w.path || w.branch || "")}">
      <span class="sicon">${BRANCH_ICON}</span><span class="stitle mono">${esc(w.branch || "(detached)")}</span>${
        (w.dirty_files || 0) > 0 ? '<span class="swt-dirty">●</span>' : ""
      }</div>`
        )
        .join("")}</div>`
    : "";
  // A collapsed list still shows where you are: the summary line takes the
  // highlight when it hides the active worktree.
  const lineActive = activeInProject && !open ? "active" : "";
  return `<div class="srow swt-line ${lineActive}" data-wtline="${esc(m.project_id)}">
    <span class="sicon">${BRANCH_ICON}</span><span class="stitle dim">${label}${dirty}</span></div>${list}`;
}

/** An outline folder, sized for the rail (the mock's project glyph). */
const FOLDER_ICON = `<svg class="sfolder" width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" aria-hidden="true"><path d="M1.75 4.25a1 1 0 0 1 1-1h3.1l1.5 1.75h6.9a1 1 0 0 1 1 1v6.75a1 1 0 0 1-1 1H2.75a1 1 0 0 1-1-1V4.25z"/></svg>`;

/** A branch glyph for the worktree rows (the mock's fork), in the FOLDER_ICON style. */
const BRANCH_ICON = `<svg class="sbranch" width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true"><circle cx="4" cy="4" r="1.7"/><circle cx="4" cy="12" r="1.7"/><circle cx="12" cy="6" r="1.7"/><path d="M4 5.7v4.6M12 7.7c0 2.3-3 2.3-5.3 2.6"/></svg>`;

/** One project's block. `ui`: { closed:Set, wtOpen:Set, activeRunId, activeProjectId } */
export function projectHtml(m, ui) {
  const open = !ui.closed.has(m.project_id);
  const badge = m.unread ? `<span class="badge sbadge">${m.unread}</span>` : "";
  // A two-line header: the project on top, its checkout's branch + status
  // beneath. Both lines survive collapsing — a shut project still says which
  // branch it has out and whether that tree is dirty.
  //
  // Three distinct targets: the chevron expands/collapses; the name opens the
  // project page (and the wiring expands the project as it navigates); the
  // checkout line opens that page's Changes tab.
  const head = `<div class="sproj-head ${ui.activeProjectId === m.project_id ? "active" : ""}">
    <button class="chevbtn" data-chev="${esc(m.project_id)}" title="${open ? "Collapse" : "Expand"}">${open ? "▾" : "▸"}</button>
    <span class="sproj-open" data-open="${esc(m.project_id)}" title="Open project">${FOLDER_ICON}<span class="sproj-name mono">${esc(m.name)}</span></span>
    ${badge}</div>${checkoutLine(m, ui)}`;
  const block = `sproj ${ui.activeProjectId === m.project_id ? "active" : ""}`.trim();
  if (!open) return `<div class="${block}">${head}</div>`;

  const active = (r) => (r.run_id === ui.activeRunId ? "active" : "");
  const activePlan = (p) => (p.plan_id === ui.activePlanId ? "active" : "");
  const needs = m.needsYou
    .map((r) =>
      runRow(r, {
        icon: r.state === "blocked" || r.state === "failed" ? '<span class="warn">▲</span>' : "✦",
        right: statHtml(r.stat),
        cls: `attn ${active(r)}`,
      })
    )
    .join("");
  const running = m.running
    .map((r) =>
      runRow(r, {
        icon: "●",
        right:
          (r.last_error ? '<span class="warn">▲</span> ' : "") +
          (r.stat && r.stat.files_changed ? statHtml(r.stat) : `<span class="sstate">${esc(r.state)}</span>`),
        cls: `work ${active(r)}`,
      })
    )
    .join("");
  // Plans (project-scoped): the ones needing you carry the attn styling and the
  // review/parked glyph; approved plans are ready to Implement; drafting plans
  // are still being authored.
  const planNeeds = m.planNeedsYou
    .map((p) =>
      planRow(p, {
        icon: p.state === "blocked" || p.state === "failed" ? '<span class="warn">▲</span>' : "✦",
        right: openCommentsRight(p),
        cls: `attn ${activePlan(p)}`,
      })
    )
    .join("");
  const planReady = m.planApproved.map((p) => planRow(p, { icon: "◆", right: openCommentsRight(p), cls: `ready ${activePlan(p)}` })).join("");
  const planDrafting = m.planDrafting.map((p) => planRow(p, { icon: "✎", cls: `work ${activePlan(p)}` })).join("");
  const done = m.doneRecently
    .map((r) =>
      runRow(r, {
        icon: doneIcon(r),
        right: `<span class="sage">${humanAge(r.age_s)}</span>`,
        cls: "done",
      })
    )
    .join("");
  const plansDone = m.plansDone
    .map((p) => planRow(p, { icon: "×", right: `<span class="sage">${humanAge(p.age_s)}</span>`, cls: "done" }))
    .join("");
  return `<div class="${block}">${head}<div class="sproj-body">
    ${section("Needs you", needs)}${section("Running", running)}
    ${section("Plans", planNeeds + planReady + planDrafting)}
    ${section("Done recently", done + plansDone)}
    ${worktreeLine(m, ui.wtOpen.has(m.project_id), ui)}</div></div>`;
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
