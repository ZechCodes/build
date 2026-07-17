// The plan surface (project-scoped review), keyed by plan_id. This stage lands
// the plumbing and a minimal, honest placeholder: the goal, live state, and the
// context banner, with Implement/Abandon affordances gated by the shared action
// rules. The full select-to-comment review + stage board is rebuilt in the next
// stage; keep this compiling against the new plan.* wire without redesign.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import { PLAN_STATE_LABEL, planChipClass, planPayloadFor } from "./shared.js";
import { canImplement, implementBlockReason, planAbandonable, planDeletable } from "../core/taskActions.js";

export async function renderPlan() {
  const root = $("#root");
  const id = App.route.id;
  let last = null;

  const goHome = () =>
    go(last && last.project_id ? { name: "project", projectId: last.project_id } : { name: "board" });

  const draw = (p) => {
    const context = planPayloadFor(p);
    const blockReason = implementBlockReason(p);
    const implementBtn = canImplement(p)
      ? '<button class="btn primary" id="implement">Implement</button>'
      : `<button class="btn primary" id="implement" disabled title="${esc(blockReason || "")}">Implement</button>`;
    const removeBtn = planDeletable(p.state)
      ? '<button class="btn danger" id="planremove">Delete</button>'
      : planAbandonable(p.state)
        ? '<button class="btn" id="planremove">Abandon</button>'
        : "";
    root.innerHTML = `
      <div class="board-head"><div>
          <h1>Plan: ${esc(p.goal)}</h1>
          <p class="mono projmeta">${esc(p.project || "")}${p.base_branch ? ` · ${esc(p.base_branch)}` : ""}</p></div>
        <span class="chip ${planChipClass(p.state)}" style="margin-left:auto">${PLAN_STATE_LABEL[p.state] || p.state}</span></div>
      ${p.last_error ? `<div class="cerr">⚠ ${esc(p.last_error)}</div>` : ""}
      ${context ? `<div class="payload">${esc(context)}</div>` : ""}
      <div class="empty">The plan review surface arrives in the next update. ${
        blockReason ? esc(blockReason) : "This plan is approved and ready to implement."
      }</div>
      <div class="actionbar"><span class="hint" id="planhint"></span><div class="right">${implementBtn}${removeBtn}</div></div>`;

    const impl = $("#implement");
    if (impl && !impl.disabled)
      impl.onclick = async () => {
        impl.disabled = true;
        impl.textContent = "starting…";
        try {
          const run = await App.call("run.create", { plan_id: id });
          go({ name: "task", id: run.run_id, tab: "changes" });
        } catch (e) {
          impl.disabled = false;
          impl.textContent = "Implement";
          $("#planhint").textContent = "error: " + e.message.slice(0, 60);
        }
      };

    const remove = $("#planremove");
    if (remove)
      remove.onclick = async () => {
        const deleting = planDeletable(p.state);
        if (!deleting && !window.confirm("Abandon this plan? Its planning worktree is removed; the plan stays as history."))
          return;
        remove.disabled = true;
        try {
          await App.call(deleting ? "plan.delete" : "plan.abandon", { plan_id: id });
          if (deleting) goHome();
          else paint();
        } catch (e) {
          remove.disabled = false;
          $("#planhint").textContent = "error: " + e.message.slice(0, 60);
        }
      };
  };

  const paint = async () => {
    if (App.offline) return;
    let p;
    try {
      p = await App.call("plan.get", { plan_id: id });
    } catch {
      return; // not readable yet — the poll retries
    }
    last = p;
    draw(p);
  };

  await paint();
  App.poll = setInterval(paint, 1600);
}
