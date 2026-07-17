// One project's page: its tasks in the board buckets, its external worktrees,
// and a New task that is pre-scoped to the project.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import {
  RUN_STATE_LABEL,
  RUN_TERMINAL_STATES,
  runChipClass,
  runPayloadFor,
  setBadge,
  PLAN_STATE_LABEL,
  planChipClass,
  planPayloadFor,
} from "./shared.js";
import { bucketPlans } from "../core/planRail.js";
import { externalWorktreeCard } from "../core/worktreeCards.js";
import { openNewTask } from "../sheets/newTask.js";

export async function renderProject() {
  const root = $("#root");
  const projectId = App.route.projectId;

  const draw = (project, runs, externalWorktrees, primaryChanges, plans) => {
    const mine = runs.filter((t) => t.project_id === projectId);
    const myPlans = (plans || []).filter((p) => p.project_id === projectId);
    const planByBucket = bucketPlans(myPlans);
    const worktrees = externalWorktrees.filter((w) => w.project_id === projectId);
    const primary = (primaryChanges || []).find((c) => c.project_id === projectId) || null;
    const byBucket = { attn: [], work: [], done: [] };
    for (const t of mine) {
      if (RUN_TERMINAL_STATES.has(t.state)) byBucket.done.push(t);
      else if (t.needs_attention) byBucket.attn.push(t);
      else byBucket.work.push(t);
    }
    // A plan card: goal, state chip, and a footer summarising its manifest —
    // stage count with the open-comment total (single-doc plans show neither).
    const planCard = (p, quiet) => {
      const stageCount = (p.stages || []).length;
      const openComments = (p.stages || []).reduce((n, s) => n + (s.open_comments || 0), 0);
      const context = planPayloadFor(p);
      const meta = [
        stageCount ? `${stageCount} stage${stageCount === 1 ? "" : "s"}` : null,
        openComments ? `${openComments} 💬` : null,
      ].filter(Boolean);
      return `
      <div class="card ${quiet ? "quiet" : ""}" data-plan="${esc(p.plan_id)}">
        <div class="top"><span class="title">Plan: ${esc(p.goal)}</span>
          <span class="chip ${planChipClass(p.state)}">${PLAN_STATE_LABEL[p.state] || p.state}</span></div>
        ${meta.length ? `<div class="meta">${meta.map((m) => `<span>${esc(m)}</span>`).join("<span>·</span>")}</div>` : ""}
        ${context ? `<div class="payload">${esc(context)}</div>` : ""}
        ${p.last_error ? `<div class="cerr">⚠ ${esc(p.last_error)}</div>` : ""}
      </div>`;
    };
    const planBucket = (label, items, quiet) =>
      items.length
        ? `<div class="bucket"><h2>${label} <span class="n">${items.length}</span></h2>${items.map((p) => planCard(p, quiet)).join("")}</div>`
        : "";
    const card = (t, quiet) => `
      <div class="card ${quiet ? "quiet" : ""}" data-id="${t.run_id}">
        <div class="top"><span class="title">${esc(t.goal)}</span>
          <span class="chip ${runChipClass(t.state)}">${RUN_STATE_LABEL[t.state] || t.state}</span></div>
        <div class="meta"><span>${esc(t.branch)}</span><span>·</span><span>${esc(t.harness)}</span></div>
        ${runPayloadFor(t) ? `<div class="payload">${esc(runPayloadFor(t))}</div>` : ""}
        ${t.last_error ? `<div class="cerr">⚠ ${esc(t.last_error)}</div>` : ""}
      </div>`;
    const bucket = (label, items, quiet) =>
      items.length
        ? `<div class="bucket"><h2>${label} <span class="n">${items.length}</span></h2>${items.map((t) => card(t, quiet)).join("")}</div>`
        : "";
    const worktreeBucket = worktrees.length
      ? `<div class="bucket"><h2>WORKTREES <span class="n">${worktrees.length}</span></h2>${worktrees.map((w) => externalWorktreeCard(w)).join("")}</div>`
      : "";
    // The primary checkout as one card: branch, path, and a dirty +/− when it has
    // uncommitted changes. Links to the main surface (#/main/<projectId>).
    const dirty = primary && primary.files_changed
      ? `<span class="pm"><span class="a">+${primary.insertions || 0}</span> <span class="d">−${primary.deletions || 0}</span></span>`
      : "";
    const mainBucket = primary
      ? `<div class="bucket"><h2>MAIN</h2>
          <div class="card quiet" data-main="${esc(projectId)}">
            <div class="top"><span class="title mono">${esc(primary.branch)}</span>${dirty}</div>
            <div class="meta"><span>${esc(project ? project.path : "")}</span></div></div></div>`
      : "";
    const anyContent = mine.length || myPlans.length || worktrees.length || primary;
    root.innerHTML = `
      <div class="board-head"><div>
          <h1>${esc(project ? project.name : projectId)}</h1>
          <p class="mono projmeta">${esc(project ? project.path : "")}${project ? ` · ${esc(project.base_branch)}` : ""}</p></div>
        <div class="row" style="margin-left:auto;gap:8px">
          <button class="btn" id="newquick">Quick task</button>
          <button class="btn primary" id="newplan">+ New plan</button></div></div>
      ${anyContent ? "" : '<div class="empty">Nothing here yet — author a plan or start a quick task.</div>'}
      ${planBucket("DRAFT", planByBucket.draft, false)}
      ${planBucket("IN REVIEW", planByBucket.review, false)}
      ${planBucket("APPROVED", planByBucket.approved, false)}
      ${bucket("NEEDS YOU", byBucket.attn, false)}
      ${bucket("WORKING", byBucket.work, true)}
      ${bucket("DONE", byBucket.done, true)}
      ${planBucket("PLAN HISTORY", planByBucket.history, true)}
      ${mainBucket}
      ${worktreeBucket}`;
    $("#newplan").onclick = () => openNewTask({ projectId, mode: "plan" });
    $("#newquick").onclick = () => openNewTask({ projectId, mode: "quick" });
    root.querySelectorAll(".card[data-plan]").forEach((c) => (c.onclick = () => go({ name: "plan", id: c.dataset.plan, tab: "review" })));
    root.querySelectorAll(".card[data-id]").forEach((c) => (c.onclick = () => go({ name: "task", id: c.dataset.id, tab: "changes" })));
    root
      .querySelectorAll(".card[data-wt]")
      .forEach((c) => (c.onclick = () => go({ name: "worktree", projectId: c.dataset.project, worktreeId: c.dataset.wt })));
    root
      .querySelectorAll(".card[data-main]")
      .forEach((c) => (c.onclick = () => go({ name: "main", projectId: c.dataset.main })));
    setBadge(runs, plans);
  };

  const load = async () => {
    try {
      const [board, projectList] = await Promise.all([App.call("board.list"), App.call("project.list")]);
      const project = (projectList.projects || []).find((p) => p.project_id === projectId) || null;
      draw(project, board.runs || [], board.external_worktrees || [], board.primary_changes || [], board.plans || []);
    } catch {
      /* offline / transient — the poll retries */
    }
  };
  await load();
  App.poll = setInterval(load, 2500);
}
