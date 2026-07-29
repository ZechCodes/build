// A project's Inbox tab: its plans, tasks, and external worktrees. The shared
// project surface owns the tab bar and persistent creation actions; this module
// only mounts the Inbox body and its polling lifecycle.

import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import {
  RUN_STATE_LABEL,
  runChipClass,
  runPayloadFor,
  setBadge,
  PLAN_STATE_LABEL,
  planChipClass,
  planPayloadFor,
} from "./shared.js";
import { bucketProjectEntities } from "../core/board.js";
import { runProgressFacts } from "../core/progressFacts.js";
import { defaultRunTab } from "../core/taskActions.js";
import { externalWorktreeCard } from "../core/worktreeCards.js";

export function mountProjectInbox(host, { projectId, callRpc = (method, params) => App.call(method, params) }) {
  let disposed = false;
  const draw = (runs, externalWorktrees, plans) => {
    const mine = runs.filter((t) => t.project_id === projectId);
    const myPlans = (plans || []).filter((p) => p.project_id === projectId);
    const entities = bucketProjectEntities({ runs: mine, plans: myPlans });
    const worktrees = externalWorktrees.filter((w) => w.project_id === projectId);
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
        <div class="top"><span class="title">Issue: ${esc(p.goal)}</span>
          <span class="chip ${planChipClass(p.state)}">${PLAN_STATE_LABEL[p.state] || p.state}</span></div>
        ${meta.length ? `<div class="meta">${meta.map((m) => `<span>${esc(m)}</span>`).join("<span>·</span>")}</div>` : ""}
        ${context ? `<div class="payload${p.state === "drafting" ? " live-claim" : ""}">${esc(context)}</div>` : ""}
        ${p.last_error ? `<div class="cerr">⚠ ${esc(p.last_error)}</div>` : ""}
      </div>`;
    };
    const planBucket = (label, items, quiet) =>
      items.length
        ? `<div class="bucket"><h2>${label} <span class="n">${items.length}</span></h2>${items.map((p) => planCard(p, quiet)).join("")}</div>`
        : "";
    // withFacts: NEEDS YOU / WORKING cards carry the live progress-facts line
    // (diffstat, last activity, time-in-state); DONE cards stay quiet history.
    const card = (t, quiet, withFacts) => {
      const facts = withFacts ? runProgressFacts(t, Date.now()) : "";
      return `
      <div class="card ${quiet ? "quiet" : ""}" data-id="${t.run_id}" data-tab="${defaultRunTab(t)}">
        <div class="top"><span class="title">${esc(t.goal)}</span>
          <span class="chip ${runChipClass(t.state)}">${RUN_STATE_LABEL[t.state] || t.state}</span></div>
        <div class="meta"><span>${esc(t.branch)}</span><span>·</span><span>${esc(t.harness)}</span></div>
        ${runPayloadFor(t) ? `<div class="payload${t.state === "building" ? " live-claim" : ""}">${esc(runPayloadFor(t))}</div>` : ""}
        ${facts ? `<div class="facts mono">${esc(facts)}</div>` : ""}
        ${t.last_error ? `<div class="cerr">⚠ ${esc(t.last_error)}</div>` : ""}
      </div>`;
    };
    // A mixed NEEDS YOU / WORKING / DONE bucket dispatches each kind-tagged entry
    // to the matching card template.
    const entryCard = (entry, quiet, withFacts) =>
      entry.kind === "run" ? card(entry.r, quiet, withFacts) : planCard(entry.p, quiet);
    const mixedBucket = (label, entries, quiet, withFacts) =>
      entries.length
        ? `<div class="bucket"><h2>${label} <span class="n">${entries.length}</span></h2>${entries.map((e) => entryCard(e, quiet, withFacts)).join("")}</div>`
        : "";
    // NEEDS YOU always renders — its emptiness is the affirmation that nothing
    // is waiting on the user, not an absent section.
    const needsYouBucket = `<div class="bucket"><h2>NEEDS YOU${
      entities.needsYou.length ? ` <span class="n">${entities.needsYou.length}</span>` : ""
    }</h2>${
      entities.needsYou.length
        ? entities.needsYou.map((e) => entryCard(e, false, true)).join("")
        : '<div class="allclear">✓ Nothing needs you.</div>'
    }</div>`;
    const worktreeBucket = worktrees.length
      ? `<div class="bucket"><h2>WORKTREES <span class="n">${worktrees.length}</span></h2>${worktrees.map((w) => externalWorktreeCard(w)).join("")}</div>`
      : "";
    const anyContent = mine.length || myPlans.length || worktrees.length;
    // Two columns when the pane is wide enough for both, one when it is not.
    // The split is a CONTAINER query, not a viewport one: collapsing the rail
    // changes this pane's width without moving the viewport an inch, and the
    // question here is only ever "does this pane have room".
    host.innerHTML = `<div class="project-inbox">
      <div class="inbox-cols">
        <div class="inbox-main">
          ${anyContent ? "" : '<div class="empty">Nothing here yet — file an issue to get started.</div>'}
          ${needsYouBucket}
          ${mixedBucket("WORKING", entities.working, true, true)}
          ${planBucket("READY TO IMPLEMENT", entities.readyPlans, false)}
          ${mixedBucket("DONE", entities.done, true, false)}
        </div>
        ${worktreeBucket ? `<aside class="inbox-side">${worktreeBucket}</aside>` : ""}
      </div></div>`;
    host.querySelectorAll(".card[data-plan]").forEach((c) =>
      (c.onclick = () => go({ name: "plan", projectId, id: c.dataset.plan, tab: "stages" })));
    host.querySelectorAll(".card[data-id]").forEach((c) =>
      (c.onclick = () => go({ name: "task", projectId, id: c.dataset.id, tab: c.dataset.tab })));
    host
      .querySelectorAll(".card[data-wt]")
      .forEach((c) => (c.onclick = () => go({ name: "worktree", projectId: c.dataset.project, worktreeId: c.dataset.wt })));
    setBadge(runs, plans);
  };

  const load = async () => {
    try {
      const board = await callRpc("board.list");
      if (!disposed) draw(board.runs || [], board.external_worktrees || [], board.plans || []);
    } catch {
      /* offline / transient — the poll retries */
    }
  };
  load();
  const poll = setInterval(load, 2500);
  return {
    dispose() {
      disposed = true;
      clearInterval(poll);
    },
  };
}
