// Notifications: every actionable plan or run state becomes one decision-carrying
// card. Plan-review and parked-planning cards route to the plan cockpit; diff
// review and parked-build cards route to the run.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { renderMarkdown } from "../core/markdown.js";
import { App, go } from "../app.js";
import { persistReadIds } from "../core/readState.js";
import { attnRuns, attnPlans, setBadge } from "./shared.js";
import { defaultRunTab } from "../core/taskActions.js";
import { notifActionsFor } from "../core/notifActions.js";

// A run's decision card. Runs never carry plan_review — that belongs to a plan.
function runNotifEvent(t) {
  if (t.state === "review")
    return { icon: "✦", tone: "", title: "Build ready to review", sub: t.summary || "The coding agent finished. Review the diff.", action: "Review diff", primary: true };
  if (t.state === "stage_gate")
    return { icon: "✦", tone: "", title: "Stage passed — next stage ready", sub: t.summary || "A stage finished and passed validation. Start the next stage.", action: "View stages", primary: true };
  if (t.state === "blocked")
    return { icon: "▲", tone: "warn", title: "Build blocked", sub: t.summary || "The agent needs your input to continue.", action: "View task", primary: false };
  if (t.state === "failed")
    return { icon: "▲", tone: "warn", title: "Build failed", sub: t.summary || "The build did not complete.", action: "View task", primary: false };
  if (t.state === "idle_unreported")
    return { icon: "▲", tone: "warn", title: "Build idle", sub: t.summary || "The build session went idle without reporting.", action: "View task", primary: false };
  if (t.state === "interrupted")
    return { icon: "▲", tone: "warn", title: "Build interrupted", sub: t.summary || "The build session was interrupted.", action: "View task", primary: false };
  if (t.state === "merged")
    return { icon: "✓", tone: "muted", title: "Merge complete", branch: `${t.branch} → ${t.base_branch || "main"}`, action: "View task", primary: false };
  return null;
}

// A plan's decision card. plan_review is the human gate; the parked arms all need
// the user to move the plan forward.
function planNotifEvent(p) {
  if (p.state === "plan_review")
    return { icon: "✦", tone: "", title: "Plan ready to review", sub: p.summary || "The planning agent finished. Review the plan.", action: "Review plan", primary: true };
  if (p.state === "blocked")
    return { icon: "▲", tone: "warn", title: "Planning blocked", sub: p.summary || "The planning agent needs your input.", action: "View plan", primary: false };
  if (p.state === "failed")
    return { icon: "▲", tone: "warn", title: "Planning failed", sub: p.summary || "Planning did not complete.", action: "View plan", primary: false };
  if (p.state === "idle_unreported")
    return { icon: "▲", tone: "warn", title: "Planning idle", sub: p.summary || "The planning session went idle without reporting.", action: "View plan", primary: false };
  if (p.state === "interrupted")
    return { icon: "▲", tone: "warn", title: "Planning interrupted", sub: p.summary || "The planning session was interrupted.", action: "View plan", primary: false };
  return null;
}

export async function renderNotifications() {
  const root = $("#root");
  const expanded = new Set(); // ids whose message is expanded (survives polling)
  const draw = (runs, plans) => {
    setBadge(runs, plans);
    const events = [
      ...runs.map((t) => ({ kind: "run", id: t.run_id, goal: t.goal, terminal: t.state === "merged", src: t, e: runNotifEvent(t) })),
      ...plans.map((p) => ({ kind: "plan", id: p.plan_id, goal: p.goal, terminal: false, src: p, e: planNotifEvent(p) })),
    ].filter((x) => x.e);
    events.sort((a, b) => a.terminal - b.terminal); // completed items sink
    // Each action descriptor becomes a button tagged data-<kind> so the wiring can
    // route open / message / open-agent independently. The open action keeps the
    // primary emphasis; act-now controls are quiet mini buttons.
    const actionBtn = (a, id) => {
      const cls = a.kind === "open" ? (a.primary ? "btn primary" : "btn") : "btn mini";
      return `<button class="${cls}" data-${a.kind}="${esc(id)}">${a.label}</button>`;
    };
    const card = ({ kind, id, goal, e }) => `
      <div class="ncard ${e.primary && App.readIds.has(id) ? "read" : ""}" data-id="${esc(id)}">
        <div class="nhead ${e.tone}"><span class="nicon">${e.icon}</span><span>${e.title}</span></div>
        <div class="ngoal">${kind === "plan" ? "Issue: " : ""}${esc(goal)}</div>
        ${e.sub ? `<div class="nmsg">${renderMarkdown(e.sub)}</div><div class="nmsg-toggle">Show more</div>` : ""}
        ${e.branch ? `<div class="nbranch">${esc(e.branch)}</div>` : ""}
        <div class="nact">${notifActionsFor(kind, e).map((a) => actionBtn(a, id)).join("")}</div>
      </div>`;
    const hasUnread =
      attnRuns(runs).some((t) => !App.readIds.has(t.run_id)) || attnPlans(plans).some((p) => !App.readIds.has(p.plan_id));
    root.innerHTML = `
      <div class="board-head"><div><h1>Notifications</h1><p>Every card carries its decision. No dead-end pings.</p></div>
        ${hasUnread ? '<button class="btn" id="markread" style="margin-left:auto">Mark all read</button>' : ""}</div>
      ${
        events.length
          ? events.map(card).join("")
          : '<div class="allclear big">✓ Nothing needs you.<span class="allclear-sub">Agents will report here when something does.</span></div>'
      }`;
    // Route by entity kind: the open action goes to the run's default tab / the
    // issue's review surface; open-agent drops straight into the live PTY. There
    // is no message action here — talking to an agent belongs to the surfaces
    // that host one, where the conversation is.
    const byId = new Map(events.map((x) => [x.id, x]));
    root.querySelectorAll(".nact button[data-open]").forEach((b) => {
      b.onclick = () => {
        const x = byId.get(b.dataset.open);
        if (!x) return;
        if (x.kind === "plan") go({ name: "plan", projectId: x.src.project_id, id: x.id, tab: "stages" });
        else go({ name: "task", projectId: x.src.project_id, id: x.id, tab: defaultRunTab(x.src) });
      };
    });
    root.querySelectorAll(".nact button[data-agent]").forEach((b) => {
      b.onclick = () => {
        const x = byId.get(b.dataset.agent);
        if (!x) return;
        go({ name: x.kind === "plan" ? "plan" : "task", projectId: x.src.project_id, id: x.id, tab: "agent" });
      };
    });
    const markRead = $("#markread");
    if (markRead)
      markRead.onclick = () => {
        attnRuns(runs).forEach((t) => App.readIds.add(t.run_id));
        attnPlans(plans).forEach((p) => App.readIds.add(p.plan_id));
        persistReadIds(App.readIds, localStorage);
        draw(runs, plans);
      };
    // Measure each message (collapsed) and only offer expand when it overflows.
    root.querySelectorAll(".ncard").forEach((cardEl) => {
      const msg = cardEl.querySelector(".nmsg");
      const toggle = cardEl.querySelector(".nmsg-toggle");
      if (!msg || !toggle) return;
      const id = cardEl.dataset.id;
      if (msg.scrollHeight > msg.clientHeight + 2) {
        toggle.style.display = "block";
        const apply = () => {
          const isExpanded = expanded.has(id);
          msg.classList.toggle("expanded", isExpanded);
          toggle.textContent = isExpanded ? "Show less" : "Show more";
        };
        apply();
        toggle.onclick = () => {
          expanded.has(id) ? expanded.delete(id) : expanded.add(id);
          apply();
        };
      } else {
        expanded.delete(id);
      }
    });
  };
  const load = async () => {
    try {
      const board = await App.call("board.list");
      draw(board.runs || [], board.plans || []);
    } catch {
      /* offline / transient — the poll retries */
    }
  };
  await load();
  App.poll = setInterval(load, 2500);
}
