// One unified project surface. Conversation, Changes, Files, Agent, and user
// terminals operate on the project's primary checkout — the repo root, adopted
// as a super-worktree the same way an external worktree is. The project-wide
// entries (Inbox, Issues, Archive, project settings) ride the tab bar's shared right
// cluster, as they do on every other project surface.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import { hashFromRoute } from "../core/router.js";
import { mountTabShell } from "../core/tabshell.js";
import { terminalTabsController, mountAuxTab, mountAgentTab, AGENT_TAB, NEW_TAB_KINDS } from "../core/surfaceTabs.js";
import { mountGitPane } from "../core/gitPane.js";
import { createPrimaryAdoptingCall, startAdoptedAgent } from "../core/adoption.js";
import { createThreadCache, threadHtml, wireThreadComposer, wireThreadLinks } from "../core/thread.js";
import { subscribeFeed, primaryRunIdFor } from "../core/taskFeed.js";
import { RUN_TERMINAL_STATES } from "../core/board.js";
import { RUN_STATE_LABEL, runChipClass } from "./shared.js";
import { notifyError } from "../core/notify.js";
import { isProjectClusterTab, mountProjectClusterTab, projectClusterShellOptions } from "../core/projectCluster.js";

/** The project surface's tabs, in row order. The primary checkout is a worktree
 *  like any other, so it carries the same Conversation and Agent fixtures — the
 *  one agent that can run in the repo root and the thread it reports into,
 *  always reachable, never started by opening them. */
export const projectSurfaceTabs = (terminalTabs = []) => [
  { id: "conversation", label: "Conversation" },
  { id: "changes", label: "Changes" },
  { id: "files", label: "Files" },
  AGENT_TAB,
  ...terminalTabs,
];

const PRIMARY_COMPOSER_IDS = { input: "mainthreadinput", send: "mainthreadsend", hint: "mainthreadhint" };

/** The primary checkout's conversation: the thread of the run that owns the
 *  repo root, over an owner this surface may not have minted yet.
 *
 *  Un-adopted it is a composer alone, and the first message is what adopts —
 *  the same adopt-on-first-mutation an external worktree gets, for the same
 *  reason (a message is addressed to an agent, and an agent needs an owner for
 *  `done` to report to). ONE difference from the external surface: nothing hands
 *  off afterwards, because the primary run's home IS this surface, so the thread
 *  the adopt opened renders in place.
 *
 *  Mounting adopts nothing. `adopting` may already be bound to the run that owns
 *  the checkout (seeded from the feed), in which case this is a plain live
 *  thread. */
export function mountPrimaryConversation(
  host,
  { adopting, callRpc, readDraft = () => "", writeDraft = () => {}, openLink = () => {}, pollMs = 1600 },
) {
  const threadCache = createThreadCache();
  let renderKey = null;
  let disposed = false;

  const composerFor = (placeholder) => ({
    inputId: PRIMARY_COMPOSER_IDS.input,
    sendId: PRIMARY_COMPOSER_IDS.send,
    hintId: PRIMARY_COMPOSER_IDS.hint,
    placeholder,
  });

  const wireComposer = ({ onSubmit, afterSubmit }) =>
    wireThreadComposer(host, {
      ids: PRIMARY_COMPOSER_IDS,
      readDraft,
      writeDraft,
      onSubmit,
      afterSubmit,
      onError: (error) => notifyError("Message failed", error.message),
    });

  // Nobody owns the checkout yet: an empty timeline and the box that mints the
  // owner. Sending adopts, posts, and stays — refresh() paints what it opened.
  const paintInvitation = () => {
    renderKey = null;
    host.innerHTML = threadHtml({ items: [] }, {
      composer: composerFor("Send a message to adopt this checkout and start its agent…"),
    });
    wireComposer({
      onSubmit: async (message) => {
        // No provider named: the checkout has no sheet answer to carry (only the
        // Agent tab's cards ask that question here), so the run is minted on the
        // bridge's default and the cards switch it later.
        const runId = await adopting.adopt();
        return callRpc("thread.post", { entity_id: runId, body: message });
      },
      afterSubmit: () => refresh(),
    });
  };

  const paintThread = (runId, view) => {
    const key = JSON.stringify({
      state: view.state,
      harness: view.harness || "",
      sessions: view.thread?.sessions || [],
      items: view.thread?.items || [],
    });
    if (key === renderKey && host.querySelector(".review-thread")) return;
    renderKey = key;
    host.innerHTML = threadHtml(view.thread, {
      agentLabel: view.harness,
      status: { label: RUN_STATE_LABEL[view.state] || view.state || "", cls: runChipClass(view.state) },
      composer: composerFor("Send a message to the coding agent…"),
    });
    wireThreadLinks(host, openLink);
    wireComposer({
      onSubmit: (message) => callRpc("thread.post", { entity_id: runId, body: message }),
      afterSubmit: () => refresh(),
    });
  };

  // The run let go of the checkout (terminal, or deleted out from under us): the
  // repo root is adoptable again, so drop the binding and offer the invitation.
  const releaseCheckout = () => {
    adopting.releaseAdoptedRun();
    threadCache.reset();
    paintInvitation();
  };

  const refresh = async () => {
    const runId = adopting.adoptedRunId();
    if (disposed || !runId) return;
    let view;
    try {
      view = await callRpc("run.get", { run_id: runId, ...threadCache.cursorParam() });
    } catch (e) {
      if (/unknown run_id/.test((e && e.message) || "")) releaseCheckout();
      return; // every other failure is transient — the next tick retries
    }
    if (disposed) return;
    if (RUN_TERMINAL_STATES.has(view.state)) {
      releaseCheckout();
      return;
    }
    paintThread(runId, { ...view, thread: threadCache.absorb(view.thread) });
  };

  if (adopting.adoptedRunId()) refresh();
  else paintInvitation();
  const timer = pollMs > 0 ? setInterval(refresh, pollMs) : null;

  return {
    refresh,
    dispose() {
      disposed = true;
      if (timer) clearInterval(timer);
    },
  };
}

export async function renderMain() {
  const root = $("#root");
  const projectId = App.route.projectId;
  const scope = { project_id: projectId };
  const terminals = terminalTabsController(scope);

  let tab = App.route.tab || "conversation";
  let projectName = projectId;
  let shellCtl = null; // tab-row controller
  let aux = null; // current changes/files/terminal pane controller
  let meta = { branch: "", path: "" };
  let viewDisposed = false; // set on navigation — stale RPC responses must not touch #root
  let conversationDraft = "";
  let linkedFilePath = null; // a file link in the conversation, opened by the Files tab

  // The primary checkout adopts exactly like an external worktree: the first
  // mutating verb here (a message, a start) mints the run that owns the repo
  // root. Opening the surface must not — mounting is a look — so the existing
  // owner is LEARNED instead, off the feed the sidebar already polls, and the
  // caller binds to it. The bridge allows one owner per project, so a reload or
  // a second browser converges on the same run either way.
  const adopting = createPrimaryAdoptingCall((method, params) => App.call(method, params), projectId);
  const unsubscribeFeed = subscribeFeed((feed) => {
    adopting.seedAdoptedRun(primaryRunIdFor(feed, projectId));
  });

  const disposeAux = () => {
    if (aux) {
      aux.dispose();
      aux = null;
    }
  };

  const staticTabs = () => projectSurfaceTabs(terminals.tabs());

  // The tab bar is the top of the view; the checkout's branch rides the bar's
  // right cluster (path on hover) and stays live via refreshHeader.
  const shell = () => {
    root.innerHTML = `
      <div class="surface-bar project-surface">
        <div class="tabrow" id="tabrow"></div>
        <div class="surface-meta">
          <span class="mono dim" id="mainbranch" title="${esc(meta.path || "")}">${esc(meta.branch || "(detached)")}</span>
        </div>
      </div>
      <div id="tabbody"></div>`;
    shellCtl = mountTabShell($("#tabrow"), {
      tabs: staticTabs(),
      active: tab,
      onSelect: (id) => selectTab(id),
      onClose: (id) => closeTerminal(id),
      newTabOptions: NEW_TAB_KINDS,
      onNewTab: () => newTerminal(),
      ...projectClusterShellOptions({ projectId, selectTab: (id) => selectTab(id) }),
    });
  };

  // A reference in the conversation: a file opens in this checkout's Files tab,
  // a plan or run opens its own surface.
  const openConversationLink = (link) => {
    if (link.kind === "file" && link.path) {
      linkedFilePath = link.path;
      selectTab("files");
      return;
    }
    const planId = link.plan_id || link.issue_id;
    if ((link.kind === "issue_stage" || link.kind === "plan_stage") && planId && link.stage_id) {
      go({ name: "plan", projectId, id: planId, tab: "stages", stage: link.stage_id });
      return;
    }
    const runId = link.implementation_id || link.run_id;
    if ((link.kind === "implementation" || link.kind === "run") && runId) {
      go({ name: "task", projectId, id: runId, tab: "conversation" });
    }
  };

  const selectTab = (id) => {
    tab = id;
    App.route = { name: "project", projectId, tab: id };
    history.replaceState(null, "", hashFromRoute(App.route));
    if (shellCtl) shellCtl.setActive(id);
    disposeAux();
    const body = $("#tabbody");
    // Terminal tabs go edge-to-edge; Changes/Files run flush (their own rail +
    // detail panes each scroll internally, so the body owns no padding/scroll).
    body.classList.toggle("bare", id === "agent" || /^term-/.test(id));
    body.classList.toggle("flush", id === "changes" || id === "files");
    if (id === "conversation") {
      // The thread of the run that owns this checkout. Un-adopted it is the
      // composer that adopts; the pane owns its own poll while it is mounted.
      aux = mountPrimaryConversation(body, {
        adopting,
        callRpc: (method, params) => App.call(method, params),
        readDraft: () => conversationDraft,
        writeDraft: (value) => {
          conversationDraft = value;
        },
        openLink: (link) => openConversationLink(link),
      });
    } else if (id === "agent") {
      // The primary checkout's own agent, addressed by the project scope — the
      // bridge keys it on the repo root, so it is the same agent the owning run
      // starts. Mounting is a look, never a start.
      // Starting one is a mutating act on this checkout, so it adopts: the run
      // it mints is what gives the agent somewhere to report `done`, and the
      // pressed card is what it is minted on.
      aux = mountAgentTab(body, scope, {
        idleLabel: "No agent is currently running in this checkout",
        onStart: (provider) => startAdoptedAgent(adopting, provider),
      });
    } else if (isProjectClusterTab(id)) {
      // Inbox, Issues and Archive are the right cluster's, on every project
      // surface — one mounting path, whichever surface the user is standing on.
      aux = mountProjectClusterTab(body, id, {
        projectId,
        callRpc: (method, params) => App.call(method, params),
        navigate: go,
      });
    } else if (id === "changes") {
      // The git pane owns its own poll; refreshHeader rides its git.status
      // responses so the branch/path header stays live while it runs.
      aux = mountGitPane(body, {
        scope,
        callRpc: (method, params) =>
          App.call(method, params).then((res) => {
            if (method === "git.status") refreshHeader(res);
            return res;
          }),
        agentCommitOptions: [],
      });
    } else {
      // One-shot: a file link that landed here opens on that path, and a later
      // visit to Files opens where it left off.
      const initialPath = id === "files" ? linkedFilePath : null;
      if (initialPath) linkedFilePath = null;
      aux = mountAuxTab(body, id, {
        scope,
        callRpc: (method, params) => App.call(method, params),
        initialPath,
        onExit: () => {
          terminals.drop(id);
          if (shellCtl) shellCtl.setTabs(staticTabs());
          selectTab("changes");
        },
      });
    }
  };

  const newTerminal = async () => {
    let termId;
    try {
      termId = await terminals.create();
    } catch (e) {
      const body = $("#tabbody");
      if (body) body.innerHTML = `<div class="empty">cannot open a terminal: ${esc((e && e.message) || "error")}</div>`;
      return;
    }
    if (shellCtl) shellCtl.setTabs(staticTabs());
    selectTab(termId);
  };

  const closeTerminal = async (termId) => {
    try {
      await terminals.close(termId);
    } catch {
      /* raced with the reaper — drop the tab regardless */
    }
    if (shellCtl) shellCtl.setTabs(staticTabs());
    if (tab === termId) selectTab("changes");
  };

  // Keep the header's branch/path current from the git pane's status responses.
  // Guarded against disposal: a git.status response resolving after navigation
  // must not overwrite the next view's header (#root now belongs to it).
  const refreshHeader = (status) => {
    if (viewDisposed) return;
    const nextMeta = { branch: status.branch, path: status.path };
    if (nextMeta.branch === meta.branch && nextMeta.path === meta.path) return;
    meta = nextMeta;
    const branchEl = root.querySelector("#mainbranch");
    if (branchEl) {
      branchEl.textContent = meta.branch || "(detached)";
      branchEl.title = meta.path || "";
    }
  };

  // Fetch project identity for the persistent header (best-effort; the id and
  // live git.status metadata remain usable while offline).
  try {
    const projectList = await App.call("project.list");
    const project = (projectList.projects || []).find((p) => p.project_id === projectId);
    if (project) {
      projectName = project.name;
      meta = { branch: project.base_branch || "", path: project.path || "" };
    }
  } catch {
    /* offline — the id stands in */
  }

  // Tear down the active terminal/files pane when the user navigates away.
  App.viewDispose = () => {
    viewDisposed = true;
    unsubscribeFeed();
    disposeAux();
  };

  shell();
  await terminals.load();
  if (shellCtl) shellCtl.setTabs(staticTabs());
  selectTab(tab);
}
