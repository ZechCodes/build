// The board: NEEDS YOU / WORKING / DONE buckets, polled every 2.5s. Plans and
// runs share the buckets — a plan in review sits in NEEDS YOU beside a run ready
// to review; a plan card opens the plan route, a run card the run route on its
// right default tab.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import {
  RUN_STATE_LABEL,
  RUN_TERMINAL_STATES,
  runChipClass,
  runPayloadFor,
  PLAN_STATE_LABEL,
  PLAN_TERMINAL_STATES,
  planChipClass,
  planPayloadFor,
  setBadge,
} from "./shared.js";
import { defaultRunTab } from "../core/taskActions.js";
import { externalWorktreeCard } from "../core/worktreeCards.js";
import { openNewTask } from "../sheets/newTask.js";

export async function renderBoard() {
  const root = $("#root");
  const draw = (runs, externalWorktrees, plans) => {
    const bucketOf = (needsAttention, terminal) => (terminal ? "done" : needsAttention ? "attn" : "work");
    const byBucket = { attn: [], work: [], done: [] };
    for (const r of runs) byBucket[bucketOf(r.needs_attention, RUN_TERMINAL_STATES.has(r.state))].push({ kind: "run", r });
    for (const p of plans) byBucket[bucketOf(p.needs_attention, PLAN_TERMINAL_STATES.has(p.state))].push({ kind: "plan", p });

    const runCard = (r, quiet) => `
      <div class="card ${quiet ? "quiet" : ""}" data-id="${esc(r.run_id)}" data-tab="${defaultRunTab(r)}">
        <div class="top"><span class="title">${esc(r.goal)}</span>
          <span class="chip ${runChipClass(r.state)}">${RUN_STATE_LABEL[r.state] || r.state}</span></div>
        <div class="meta"><span>${esc(r.project)}</span><span>·</span><span>${esc(r.branch)}</span><span>·</span><span>${esc(r.harness)}</span></div>
        ${runPayloadFor(r) ? `<div class="payload">${esc(runPayloadFor(r))}</div>` : ""}
        ${r.last_error ? `<div class="cerr">⚠ ${esc(r.last_error)}</div>` : ""}
      </div>`;
    // A plan card reads as a plan: "Plan: <goal>" and the plan's state palette. A
    // manifest footer (stage count + open comments) sets it apart from a run.
    const planCard = (p, quiet) => {
      const stageCount = (p.stages || []).length;
      const openComments = (p.stages || []).reduce((n, s) => n + (s.open_comments || 0), 0);
      const context = planPayloadFor(p);
      const meta = [
        p.project || null,
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
    const cardHtml = (entry, quiet) => (entry.kind === "run" ? runCard(entry.r, quiet) : planCard(entry.p, quiet));
    const bucket = (label, items, quiet) =>
      items.length
        ? `<div class="bucket"><h2>${label} <span class="n">${items.length}</span></h2>${items.map((e) => cardHtml(e, quiet)).join("")}</div>`
        : "";
    const externalBucket = externalWorktrees.length
      ? `<div class="bucket"><h2>OTHER WORKTREES <span class="n">${externalWorktrees.length}</span></h2>${externalWorktrees.map((w) => externalWorktreeCard(w)).join("")}</div>`
      : "";
    const anyContent = byBucket.attn.length || byBucket.work.length || byBucket.done.length || externalWorktrees.length;
    root.innerHTML = `
      <div class="board-head"><div><h1>Board</h1><p>What needs you — and what doesn't.</p></div>
        <button class="btn primary" id="newtask" style="margin-left:auto">+ New</button></div>
      ${anyContent ? "" : '<div class="empty">No tasks yet — author a plan or start a quick task with “New”.</div>'}
      ${bucket("NEEDS YOU", byBucket.attn, false)}
      ${bucket("WORKING", byBucket.work, true)}
      ${bucket("DONE", byBucket.done, true)}
      ${externalBucket}`;
    $("#newtask").onclick = () => openNewTask();
    // Three card kinds, three routes: run cards (data-id) open the run route on
    // their default tab; plan cards (data-plan) open the plan route; external
    // worktree cards (data-wt) open the read-only browse view.
    root.querySelectorAll(".card[data-id]").forEach((c) => (c.onclick = () => go({ name: "task", id: c.dataset.id, tab: c.dataset.tab })));
    root.querySelectorAll(".card[data-plan]").forEach((c) => (c.onclick = () => go({ name: "plan", id: c.dataset.plan, tab: "review" })));
    root
      .querySelectorAll(".card[data-wt]")
      .forEach((c) => (c.onclick = () => go({ name: "worktree", projectId: c.dataset.project, worktreeId: c.dataset.wt })));
    setBadge(runs, plans); // external worktrees never count toward the attention badge
  };
  const load = async () => {
    try {
      const res = await App.call("board.list");
      draw(res.runs || [], res.external_worktrees || [], res.plans || []);
    } catch {
      /* offline / transient — the poll retries */
    }
  };
  await load();
  App.poll = setInterval(load, 2500);
}
