// The board: NEEDS YOU / WORKING / DONE buckets, polled every 2.5s.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import { STATE_LABEL, chipClass, payloadFor, setBadge } from "./shared.js";
import { openNewTask } from "../sheets/newTask.js";

export async function renderBoard() {
  const root = $("#root");
  const draw = (tasks) => {
    const byBucket = { attn: [], work: [], done: [] };
    for (const t of tasks) {
      if (t.state === "merged" || t.state === "abandoned") byBucket.done.push(t);
      else if (t.needs_attention) byBucket.attn.push(t);
      else byBucket.work.push(t);
    }
    const card = (t, quiet) => `
      <div class="card ${quiet ? "quiet" : ""}" data-id="${t.task_id}">
        <div class="top"><span class="title">${esc(t.goal)}</span>
          <span class="chip ${chipClass(t.state)}">${STATE_LABEL[t.state] || t.state}</span></div>
        <div class="meta"><span>${esc(t.project)}</span><span>·</span><span>${esc(t.branch)}</span><span>·</span><span>${esc(t.harness)}</span></div>
        ${payloadFor(t) ? `<div class="payload">${esc(payloadFor(t))}</div>` : ""}
      </div>`;
    const bucket = (label, items, quiet) =>
      items.length
        ? `<div class="bucket"><h2>${label} <span class="n">${items.length}</span></h2>${items.map((t) => card(t, quiet)).join("")}</div>`
        : "";
    root.innerHTML = `
      <div class="board-head"><div><h1>Board</h1><p>What needs you — and what doesn't.</p></div>
        <button class="btn primary" id="newtask" style="margin-left:auto">+ New task</button></div>
      ${byBucket.attn.length || byBucket.work.length || byBucket.done.length ? "" : '<div class="empty">No tasks yet — start one with “New task”.</div>'}
      ${bucket("NEEDS YOU", byBucket.attn, false)}
      ${bucket("WORKING", byBucket.work, true)}
      ${bucket("DONE", byBucket.done, true)}`;
    $("#newtask").onclick = openNewTask;
    root.querySelectorAll(".card").forEach((c) => (c.onclick = () => go({ name: "task", id: c.dataset.id, tab: "plan" })));
    setBadge(tasks);
  };
  const load = async () => {
    try {
      draw((await App.call("task.list")).tasks);
    } catch {
      /* offline / transient — the poll retries */
    }
  };
  await load();
  App.poll = setInterval(load, 2500);
}
