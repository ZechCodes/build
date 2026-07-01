// Notifications: every actionable task state becomes one decision-carrying card.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { renderMarkdown } from "../core/markdown.js";
import { App, go } from "../app.js";
import { attnTasks, payloadFor, setBadge } from "./shared.js";

function notifEvent(t) {
  if (t.state === "plan_review")
    return { icon: "✦", tone: "", title: "Plan ready to review", sub: payloadFor(t) || "A planning agent finished drafting the plan.", action: "Review plan", tab: "plan", primary: true };
  if (t.state === "review")
    return { icon: "✦", tone: "", title: "Build ready to review", sub: t.summary || "The coding agent finished. Review the diff.", action: "Review diff", tab: "diff", primary: true };
  if (t.state === "blocked")
    return { icon: "▲", tone: "warn", title: "Build blocked", sub: t.summary || "The agent needs your input to continue.", action: "View task", tab: "diff", primary: false };
  if (t.state === "failed")
    return { icon: "▲", tone: "warn", title: "Build failed", sub: t.summary || "The build did not complete.", action: "View task", tab: "diff", primary: false };
  if (t.state === "merged")
    return { icon: "✓", tone: "muted", title: "Merge complete", branch: `${t.branch} → ${t.base_branch || "main"}`, action: "View task", tab: "diff", primary: false };
  return null;
}

export async function renderNotifications() {
  const root = $("#root");
  const expanded = new Set(); // task ids whose message is expanded (survives polling)
  const draw = (tasks) => {
    setBadge(tasks);
    const events = tasks.map((t) => ({ t, e: notifEvent(t) })).filter((x) => x.e);
    events.sort((a, b) => (a.t.state === "merged") - (b.t.state === "merged"));
    const card = ({ t, e }) => `
      <div class="ncard ${e.primary && App.readIds.has(t.task_id) ? "read" : ""}" data-id="${t.task_id}">
        <div class="nhead ${e.tone}"><span class="nicon">${e.icon}</span><span>${e.title}</span></div>
        <div class="ngoal">${esc(t.goal)}</div>
        ${e.sub ? `<div class="nmsg">${renderMarkdown(e.sub)}</div><div class="nmsg-toggle">Show more</div>` : ""}
        ${e.branch ? `<div class="nbranch">${esc(e.branch)}</div>` : ""}
        <div class="nact"><button class="btn ${e.primary ? "primary" : ""}" data-id="${t.task_id}" data-tab="${e.tab}">${e.action}</button></div>
      </div>`;
    const hasUnread = attnTasks(tasks).some((t) => !App.readIds.has(t.task_id));
    root.innerHTML = `
      <div class="board-head"><div><h1>Notifications</h1><p>Every card carries its decision. No dead-end pings.</p></div>
        ${hasUnread ? '<button class="btn" id="markread" style="margin-left:auto">Mark all read</button>' : ""}</div>
      ${events.length ? events.map(card).join("") : '<div class="empty">You’re all caught up.</div>'}`;
    root.querySelectorAll(".nact button").forEach((b) => (b.onclick = () => go({ name: "task", id: b.dataset.id, tab: b.dataset.tab })));
    const markRead = $("#markread");
    if (markRead)
      markRead.onclick = () => {
        attnTasks(tasks).forEach((t) => App.readIds.add(t.task_id));
        draw(tasks);
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
      draw((await App.call("task.list")).tasks);
    } catch {
      /* offline / transient — the poll retries */
    }
  };
  await load();
  App.poll = setInterval(load, 2500);
}
