// The issue view: a persistent two-column stages | stage viewer, and nothing
// else. An issue is a project-level work item whose children are its stages and
// the implementations that carried them out; both columns are one surface with
// no tabs and no drill-in, so reading a stage never costs the list.
//
// The conversation is not here: an agent's conversation lives in the agent rail
// beside every surface, so this view carries only the issue's own artifacts.
//
// Thin by design — the surface itself is core/issueView.js, which owns its poll,
// its repaint freeze and its wiring. This file is the route's host: it names the
// issue, keeps the URL on the open stage, and hands the view back its way home.

import { $ } from "../dom.js";
import { App, go, loadModelCatalog, markEntityRead } from "../app.js";
import { mountIssueView } from "../core/issueView.js";
import { hashFromRoute } from "../core/router.js";

export async function renderPlan() {
  const root = $("#root");
  const id = App.route.id;
  // Looking at an issue is seeing it — the dot settles until it moves again.
  App.call("entity.seen", { entity_id: id }).catch(() => {});
  markEntityRead(id);
  let projectId = App.route.projectId || null;
  let selectedStageId = App.route.stage || null;

  // The open stage rides the hash without re-routing, so the selection is
  // shareable and survives a reload.
  const syncHash = () => {
    App.route = {
      name: "plan",
      ...(projectId ? { projectId } : {}),
      id,
      tab: "stages",
      ...(selectedStageId ? { stage: selectedStageId } : {}),
    };
    history.replaceState(null, "", hashFromRoute(App.route));
  };

  root.innerHTML = '<div id="tabbody" class="flush"></div>';

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
    // The route may have arrived without a project (a bare #/issue/<id> link);
    // the first payload says which one it is, and the URL catches up.
    onProject: (id) => {
      projectId = id;
      syncHash();
    },
    onGone: () => go(projectId ? { name: "project", projectId } : { name: "notifications" }),
  });

  // The surface polls itself; App.poll holds it too, so the shell's own
  // route-change teardown stops it even before viewDispose runs.
  App.poll = view.poll;
  App.viewDispose = () => view.dispose();
}
