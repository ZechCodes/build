// The branch work item's surface: two tabs, Changes and Files, and nothing
// else — the conversation is the agent rail and the terminals are the console.
//
// The row is just tabs now. Which branch this is, which project it lives in,
// how long its agent has been working and what the diff weighs are the
// toolbar's (core/toolbar.js), one row above; the surface below states only
// what is inside it.
//
// The surface resolves what stands under the branch with `branch.get`: a run,
// a bare worktree, or the primary checkout. That resolution names the git
// scope the tab bodies read, and which review plug the Changes rail carries —
// a run's aggregate review diff (taskReview) or a bare worktree's
// adopt-on-comment diff (worktreeReview). The primary checkout browses its
// own commits with no aggregate entry.
//
// A branch name comes from the repo: untrusted, and escaped everywhere it is
// painted.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import { tabShellHtml } from "../core/tabshell.js";
import { mountConsole } from "../core/console.js";
import { mountAgentRail } from "../core/agentRail.js";
import { createAgentSelection } from "../core/agentSelection.js";
import { mountGitPane, taskAgentCommitOptions } from "../core/gitPane.js";
import { renderFilesTab } from "./files.js";
import { createTaskReview } from "./taskReview.js";
import { createWorktreeReview } from "./worktreeReview.js";
import { createAdopters } from "../core/adoption.js";
import { noteSelfAction } from "../core/inboxView.js";
import { entityIdOf } from "../core/entityId.js";
import "../styles/shell.css";
import "../styles/surfaces.css";

const BRANCH_TABS = [
  { id: "changes", label: "Changes" },
  { id: "files", label: "Files" },
];

// The cadence every work surface has always read its entity at: fast enough
// that a state flip (building → review) moves the actionbar while you watch.
const ROW_POLL_MS = 1600;

/** The git scope of what stands under the branch row: exactly one of
 *  { run_id } / { project_id, worktree_id } / { project_id } (primary), or
 *  null when the row names no checkout this device holds. Pure. */
export function branchScope(row, projectId) {
  if (!row) return null;
  if (row.run_id) return { run_id: row.run_id };
  const project = row.project_id || projectId;
  if (!project) return null;
  if (row.worktree_id) return { project_id: project, worktree_id: row.worktree_id };
  return row.primary ? { project_id: project } : null;
}

/** What the Changes rail's review plug is made for — a plug survives repaints
 *  only while this key holds, so pending comments outlive tab switches but
 *  never leak across an adoption (worktree → run). Pure. */
export function reviewKeyOf(scope) {
  if (!scope) return null;
  if (scope.run_id) return `run:${scope.run_id}`;
  if (scope.worktree_id) return `worktree:${scope.worktree_id}`;
  return null; // the primary checkout has no aggregate review entry
}

export async function renderBranch() {
  const root = $("#root");
  const { projectId, branch } = App.route;
  const tab = App.route.tab || "changes";
  root.className = "surface";
  root.innerHTML = `
    <div class="surface-bar">
      <div class="tabrow" id="branch-tabs"></div>
    </div>
    <div id="tabbody"><div class="empty">loading…</div></div>`;
  $("#branch-tabs").innerHTML = tabShellHtml({ tabs: BRANCH_TABS, active: tab });
  $("#branch-tabs")
    .querySelectorAll("[data-tab]")
    .forEach((cell) => {
      cell.onclick = () => go({ name: "branch", projectId, branch, tab: cell.dataset.tab });
    });
  // The basement, at the bottom of the view column: this branch's checkout, as
  // terminals. Shut unless the last visit left it open.
  const consolePanel = mountConsole($("#console-region"), { kind: "branch", projectId, branch });
  // The agents beside the work, not instead of it: the rail belongs to this
  // branch, so it is mounted with the surface and torn down with it. Which
  // bubble is open is the whole surface's business — the row this view reads
  // carries that agent's conversation, and the review comments Changes sends go
  // into it — so the choice lives in a handle they share.
  const agentSelection = createAgentSelection();

  let disposed = false;
  let row = null; // the branch.get payload: the feed row plus `run`
  let pane = null; // the mounted tab body ({ dispose })
  let mountedKey = null; // what the body was mounted over: tab + review key
  let reviewPlug = null; // ONE instance per backing, so pending comments survive
  let reviewKey = null;

  const callRpc = (method, params) => App.call(method, params);
  // Two surfaces here can mutate an unclaimed checkout first — the rail's first
  // message and the review's first comment or action — and near-simultaneous
  // adoptions would ask for two owners of one checkout. Both take their adopter
  // from here, so the checkout is claimed once.
  const adopterFor = createAdopters(callRpc);
  const adoptingHere = () => adopterFor(branchScope(row, projectId));

  const rail = mountAgentRail($("#agent-rail"), {
    kind: "branch",
    projectId,
    branch,
    selection: agentSelection,
    adopting: adoptingHere,
  });
  const home = () => go({ name: "inbox" });
  /** An ending the user triggered here must not badge its own inbox entry:
   *  Merged/Abandoned are attention-class, so the entry's cursor is cleared on
   *  the way out (the Stage B rule; core/inboxView.js noteSelfAction). */
  const finished = () => {
    noteSelfAction(entityIdOf(row), row && row.issue_id);
    home();
  };

  /** The plug for the Changes rail's aggregate entry, made once per backing.
   *  A run reviews through its own diff and verbs; a bare worktree adopts on
   *  the first comment or action; the primary checkout carries none. */
  const reviewFor = (scope) => {
    const key = reviewKeyOf(scope);
    if (!key) return null;
    if (key !== reviewKey) {
      reviewKey = key;
      if (scope.run_id) {
        reviewPlug = createTaskReview({
          taskId: scope.run_id,
          callRpc,
          getTask: () => (row ? row.run : null),
          isOffline: () => App.offline,
          agentSelection,
          onMerged: () => finished(),
        });
      } else {
        reviewPlug = createWorktreeReview({
          projectId: scope.project_id,
          worktreeId: scope.worktree_id,
          callRpc,
          adopting: adopterFor(scope),
          isOffline: () => App.offline,
          // Adoption keeps the URL — the same branch now stands on a run, so
          // the surface re-resolves and the Changes rail re-mounts run-backed.
          onAdopted: () => refresh(true),
          onFinished: () => finished(),
          onGone: () => refresh(true),
        });
      }
    }
    const plug = reviewPlug;
    return {
      getBase: () => (scope.run_id ? (row && row.run && row.run.base_branch) || "main" : plug.getBase()),
      mount: (host) => plug.mount(host),
      unmount: () => plug.unmount(),
    };
  };

  /** Mount the open tab's body over the resolved row. Idempotent per
   *  (tab, backing): polls repaint nothing — the panes own their own polls —
   *  so only a change of backing (adoption, worktree pruned) remounts. */
  const mountBody = () => {
    const host = $("#tabbody");
    if (!host) return;
    const scope = branchScope(row, projectId);
    const key = `${tab}:${reviewKeyOf(scope) || (scope ? "primary" : "none")}`;
    if (key === mountedKey) return;
    if (pane) {
      pane.dispose();
      pane = null;
    }
    mountedKey = key;
    if (!scope) {
      host.innerHTML = `<div class="empty">No checkout on this device carries <span class="mono">${esc(branch)}</span>.</div>`;
      return;
    }
    if (tab === "files") {
      pane = renderFilesTab(host, { scope, callRpc });
      return;
    }
    pane = mountGitPane(host, {
      scope,
      callRpc,
      agentCommitOptions: row && row.run ? taskAgentCommitOptions(row.run.state, row.run.goal) : [],
      review: reviewFor(scope),
      agentSelection,
    });
  };

  /** One read of the branch row. `force` remounts even when the backing is
   *  unchanged (an adoption just happened underneath the plug). */
  const refresh = async (force = false) => {
    let payload;
    try {
      payload = await callRpc("branch.get", { project_id: projectId, branch, ...agentSelection.scope() });
    } catch {
      // The branch stopped resolving: merged away, renamed, or the worktree is
      // gone. A row we already painted stays; a first read that fails says so.
      if (!disposed && !row) {
        const host = $("#tabbody");
        if (host)
          host.innerHTML = `<div class="empty gone">No checkout in this project carries <span class="mono">${esc(branch)}</span>.<div><button class="btn" id="branchback">Back to inbox</button></div></div>`;
        const back = $("#branchback");
        if (back) back.onclick = () => home();
        mountedKey = "gone";
      }
      return;
    }
    if (disposed) return;
    row = payload;
    if (force) mountedKey = null;
    mountBody();
  };

  App.viewDispose = () => {
    disposed = true;
    if (pane) pane.dispose();
    pane = null;
    rail.dispose();
    consolePanel.dispose();
  };
  await refresh();
  App.poll = setInterval(refresh, ROW_POLL_MS);
}
