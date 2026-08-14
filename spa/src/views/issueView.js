// The issue work item's surface: two persistent columns, the planned stages and
// the stage viewer. No tabs — the conversation is the agent rail.
//
// Thin by design — the surface itself is core/issueView.js, which owns its
// poll, its repaint freeze, the doc-comment layer and the worktree/agent
// assignment control. This file is the route's host: it names the issue, keeps
// the URL on the open stage, and mounts the rail and the console beside it.

import { $ } from "../dom.js";
import { App, go, loadModelCatalog } from "../app.js";
import { hashFromRoute } from "../core/router.js";
import { mountIssueView } from "../core/issueView.js";
import { mountConsole } from "../core/console.js";
import { mountAgentRail } from "../core/agentRail.js";
import "../styles/shell.css";
import "../styles/surfaces.css";

export async function renderIssue() {
  const root = $("#root");
  const id = App.route.id;
  let projectId = App.route.projectId || null;
  let selectedStageId = App.route.stage || null;
  root.className = "surface";
  root.innerHTML = '<div id="tabbody" class="flush"></div>';

  // Looking at an issue is seeing it — the dot settles until it moves again.
  App.call("entity.seen", { entity_id: id }).catch(() => {});

  // The open stage rides the hash without re-routing, so the selection is
  // shareable and survives a reload.
  const syncHash = () => {
    App.route = {
      name: "issue",
      ...(projectId ? { projectId } : {}),
      id,
      ...(selectedStageId ? { stage: selectedStageId } : {}),
    };
    history.replaceState(null, "", hashFromRoute(App.route));
  };

  // An issue's agent runs on the primary checkout, so that is the directory its
  // console opens terminals in.
  const consolePanel = mountConsole($("#console-region"), { kind: "issue", projectId, issueId: id });
  // An issue carries exactly one agent session, and this is where you talk to
  // it — including the first message, which is what starts it.
  const rail = mountAgentRail($("#agent-rail"), { kind: "issue", projectId, issueId: id });

  const view = mountIssueView($("#tabbody"), {
    issueId: id,
    projectId,
    initialStageId: selectedStageId,
    callRpc: (method, params) => App.call(method, params),
    navigate: go,
    loadCatalog: loadModelCatalog,
    onSelectStage: (stageId) => {
      selectedStageId = stageId;
      syncHash();
    },
    // The route may have arrived without a project (a legacy #/issue/<id>
    // link); the first payload says which one it is, and the URL catches up.
    onProject: (project) => {
      projectId = project;
      syncHash();
    },
    onGone: () => go({ name: "inbox" }),
  });

  // The surface polls itself; App.poll holds it too, so the shell's own
  // route-change teardown stops it even before viewDispose runs.
  App.poll = view.poll;
  App.viewDispose = () => {
    view.dispose();
    rail.dispose();
    consolePanel.dispose();
  };
}
