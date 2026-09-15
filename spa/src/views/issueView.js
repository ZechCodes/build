// The issue work item's surface: two persistent columns, the planned stages and
// the stage viewer. No tabs — the conversation is the agent rail.
//
// Thin by design — the surface itself is core/issueView.js, which owns its
// poll, its repaint freeze, the doc-comment layer and the worktree/agent
// assignment control. This file is the route's host: it names the issue, keeps
// the URL on the open stage, and mounts the rail and the console beside it.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { hashFromRoute } from "../core/router.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { canAnswer, routeContext } from "../core/deviceContexts.js";
import { deviceCatalog } from "../core/inboxDevices.js";
import { mountIssueView } from "../core/issueView.js";
import { mountConsole } from "../core/console.js";
import { mountAgentRail } from "../core/agentRail.js";
import { createAgentSelection } from "../core/agentSelection.js";
import "../styles/shell.css";
import "../styles/surfaces.css";

export async function renderIssue() {
  const root = $("#root");
  const id = App.route.id;
  const deviceId = App.route.deviceId || null;
  let projectId = App.route.projectId || null;
  let selectedStageId = App.route.stage || null;
  // The machine this link is about: its caller, its cache and its conversations
  // are what the surface, the rail and the console below are built on. One that
  // cannot answer — never opened here, or gone since — has nothing under this
  // link, so the surface names it instead of painting an empty issue.
  const context = routeContext(App.route);
  root.className = "surface";
  if (!canAnswer(context)) {
    mountDeviceNotice(root, deviceId); // …and hands the link back when it lands
    return;
  }
  // The device's caller, not the session's: a drop and resume under this
  // surface replaces the transport, and the surface keeps asking the machine.
  const callRpc = context.rpc;
  root.innerHTML = '<div id="tabbody" class="flush"></div>';
  // This machine answers now. If it goes while the surface is open, what was
  // read stays on screen and the strip says whose state that is.
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

  // The work item and the machine it is on — the address the console and the
  // rail are both mounted at, minted once so the two cannot drift apart.
  const workAddress = { kind: "issue", deviceId, projectId, issueId: id, call: callRpc, cacheScope: context.cacheScope };
  // An issue's agent runs on the primary checkout, so that is the directory its
  // console opens terminals in.
  const consolePanel = mountConsole($("#console-region"), { ...workAddress });
  // An issue carries exactly one agent session, and this is where you talk to
  // it — including the first message, which is what starts it. The surface
  // beside the rail reads and writes that same conversation, so both are given
  // the one handle that says which agent it is.
  const agentSelection = createAgentSelection();
  const rail = mountAgentRail($("#agent-rail"), {
    ...workAddress,
    selection: agentSelection,
    chatRepository: context.chatRepository,
  });

  const view = mountIssueView($("#tabbody"), {
    issueId: id,
    projectId,
    agentSelection,
    viewingContext: App.viewingContext,
    initialStageId: selectedStageId,
    callRpc,
    navigate: go,
    // The harnesses on offer are this machine's, asked for the way every other
    // surface asks: by the device its link names.
    loadCatalog: () => deviceCatalog(deviceId),
    // The branches an implementation can be sent into are the feed's own branch
    // rows, so the assignment control reads the same list the inbox does.
    loadWorkItems: async () => (await callRpc("board.list")).items || [],
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
    deviceStrip();
  };
}
