// One project's page: its tasks in the board buckets, its external worktrees,
// and a New task that is pre-scoped to the project.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import { STATE_LABEL, chipClass, payloadFor, setBadge } from "./shared.js";
import { externalWorktreeCard } from "../core/worktreeCards.js";
import { openNewTask } from "../sheets/newTask.js";

export async function renderProject() {
  const root = $("#root");
  const projectId = App.route.projectId;

  const draw = (project, tasks, externalWorktrees) => {
    const mine = tasks.filter((t) => t.project_id === projectId);
    const worktrees = externalWorktrees.filter((w) => w.project_id === projectId);
    const byBucket = { attn: [], work: [], done: [] };
    for (const t of mine) {
      if (t.state === "merged" || t.state === "abandoned") byBucket.done.push(t);
      else if (t.needs_attention) byBucket.attn.push(t);
      else byBucket.work.push(t);
    }
    const card = (t, quiet) => `
      <div class="card ${quiet ? "quiet" : ""}" data-id="${t.task_id}">
        <div class="top"><span class="title">${esc(t.goal)}</span>
          <span class="chip ${chipClass(t.state)}">${STATE_LABEL[t.state] || t.state}</span></div>
        <div class="meta"><span>${esc(t.branch)}</span><span>·</span><span>${esc(t.harness)}</span></div>
        ${payloadFor(t) ? `<div class="payload">${esc(payloadFor(t))}</div>` : ""}
        ${t.last_error ? `<div class="cerr">⚠ ${esc(t.last_error)}</div>` : ""}
      </div>`;
    const bucket = (label, items, quiet) =>
      items.length
        ? `<div class="bucket"><h2>${label} <span class="n">${items.length}</span></h2>${items.map((t) => card(t, quiet)).join("")}</div>`
        : "";
    const worktreeBucket = worktrees.length
      ? `<div class="bucket"><h2>WORKTREES <span class="n">${worktrees.length}</span></h2>${worktrees.map((w) => externalWorktreeCard(w)).join("")}</div>`
      : "";
    const anyContent = mine.length || worktrees.length;
    root.innerHTML = `
      <div class="board-head"><div>
          <div class="crumb"><a href="#/board">← Board</a></div>
          <h1>${esc(project ? project.name : projectId)}</h1>
          <p class="mono projmeta">${esc(project ? project.path : "")}${project ? ` · ${esc(project.base_branch)}` : ""}</p></div>
        <button class="btn primary" id="newtask" style="margin-left:auto">+ New task</button></div>
      ${anyContent ? "" : '<div class="empty">Nothing here yet — start a task in this project.</div>'}
      ${bucket("NEEDS YOU", byBucket.attn, false)}
      ${bucket("WORKING", byBucket.work, true)}
      ${bucket("DONE", byBucket.done, true)}
      ${worktreeBucket}`;
    $("#newtask").onclick = () => openNewTask({ projectId });
    root.querySelectorAll(".card[data-id]").forEach((c) => (c.onclick = () => go({ name: "task", id: c.dataset.id, tab: "plan" })));
    root
      .querySelectorAll(".card[data-wt]")
      .forEach((c) => (c.onclick = () => go({ name: "worktree", projectId: c.dataset.project, worktreeId: c.dataset.wt })));
    setBadge(tasks);
  };

  const load = async () => {
    try {
      const [list, projectList] = await Promise.all([App.call("task.list"), App.call("project.list")]);
      const project = (projectList.projects || []).find((p) => p.project_id === projectId) || null;
      draw(project, list.tasks || [], list.external_worktrees || []);
    } catch {
      /* offline / transient — the poll retries */
    }
  };
  await load();
  App.poll = setInterval(load, 2500);
}
