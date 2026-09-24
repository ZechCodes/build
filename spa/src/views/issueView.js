// The issue work item's surface: two persistent columns, the planned stages and
// the stage viewer. No tabs — the conversation is the agent rail.
//
// Thin by design — the surface itself is core/issueView.js, which owns its
// poll, its repaint freeze, the doc-comment layer and the worktree/agent
// assignment control. This file is the route's host: it names the issue and
// keeps the URL on the open stage. The rail and the console beside it are the
// shell's (core/shell.js) — this page reads the selection it has to share with
// them and mounts neither.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { hashFromRoute } from "../core/router.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { surfaceContext } from "../core/surfaceContext.js";
import { deviceFeedNow } from "../core/feedRows.js";
import { deviceCatalog } from "../core/inboxDevices.js";
import { mountIssueView } from "../core/issueView.js";
import { shellSelection } from "../core/shell.js";
import "../styles/shell.css";
import "../styles/surfaces.css";

export async function renderIssue() {
  const root = $("#root");
  const id = App.route.id;
  const deviceId = App.route.deviceId || null;
  let projectId = App.route.projectId || null;
  let selectedStageId = App.route.stage || null;
  // The machine this link is about: its caller, its cache and its conversations
  // are what the surface, the rail and the console below are built on.
  const context = surfaceContext(App.route);
  root.className = "surface";
  // The surface paints what the records hold of this machine whether or not it
  // can answer. Only a machine nothing here has ever held has nothing to paint:
  // the notice names it, waits for it, and hands the link back when it lands.
  if (!context) {
    mountDeviceNotice(root, deviceId);
    return;
  }
  // The device's caller, not the session's: a drop and resume under this
  // surface replaces the transport, and the surface keeps asking the machine.
  const callRpc = context.rpc;
  root.innerHTML = '<div id="tabbody" class="flush"></div>';
  // While the machine cannot answer, what the records hold stays on screen and
  // the strip says whose state that is.
  const deviceStrip = mountDeviceStrip(root, context);

  // Looking at an issue is seeing it — the dot settles until it moves again.
  callRpc("entity.seen", { entity_id: id }).catch(() => {});

  // The open stage rides the hash without re-routing, so the selection is
  // shareable and survives a reload.
  const syncHash = () => {
    App.route = {
      name: "issue",
      ...(deviceId ? { deviceId } : {}),
      ...(projectId ? { projectId } : {}),
      id,
      ...(selectedStageId ? { stage: selectedStageId } : {}),
    };
    history.replaceState(null, "", hashFromRoute(App.route));
  };

  // An issue carries exactly one agent session, and the rail beside this page is
  // where you talk to it — including the first message, which is what starts it.
  // The surface below reads and writes that same conversation, so it takes the
  // shell's handle for which agent it is rather than minting a second one.
  const agentSelection = shellSelection();

  const view = mountIssueView($("#tabbody"), {
    issueId: id,
    projectId,
    deviceId,
    agentSelection,
    viewingContext: App.viewingContext,
    initialStageId: selectedStageId,
    callRpc,
    navigate: go,
    // The harnesses on offer are this machine's, asked for the way every other
    // surface asks: by the device its link names.
    loadCatalog: () => deviceCatalog(deviceId),
    // The branches an implementation can be sent into are the feed's own branch
    // rows, so the assignment control reads the same list the inbox does —
    // out of the cache, which is where that list is.
    loadWorkItems: async () => deviceFeedNow(deviceId)?.items || [],
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
    deviceStrip();
  };
}
