// The branch work item's surface: two tabs, Changes and Files, and nothing
// else — the conversation is the agent rail and the terminals are the console.
//
// The surface has no bar of its own: which branch this is, and how it ends,
// are both the toolbar's now (core/toolbar.js) — the branch name because the
// nav bar already says it, Done (the same `branch.finish` the inbox row's
// Done sends) through the toolbar's verb slot (`setToolbarVerb`), so a reader
// standing IN the branch finds it beside the name it ends. Files/Changes have
// a narrow view rail between the inbox and the pane's own list (`paintTabs`) —
// #tabbody is flush against the toolbar, nothing above it spends the height.
// core/branchFinish.js decides when Done is offered and what it promises.
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
import { App, go, markRoute } from "../app.js";
import { watchChanges } from "../core/changeEvents.js";
import { paintDirectoryRail } from "../core/directoryRail.js";
import { mountConsole } from "../core/console.js";
import { setToolbarVerb, clearToolbarVerb } from "../core/toolbar.js";
import { mountAgentRail } from "../core/agentRail.js";
import { createAgentSelection } from "../core/agentSelection.js";
import { mountGitPane, taskAgentCommitOptions } from "../core/gitPane.js";
import { renderFilesTab } from "./files.js";
import { createTaskReview } from "./taskReview.js";
import { createWorktreeReview } from "./worktreeReview.js";
import { initialBranchState, projectGitState } from "./branchSeed.js";
import { createAdopters } from "../core/adoption.js";
import { INBOX_SCOPE, finishWorkItem, noteSelfAction } from "../core/inboxView.js";
import { entityIdOf } from "../core/entityId.js";
import { canAnswer, routeContext } from "../core/deviceContexts.js";
import { routeProjectKey } from "../core/deviceKey.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { mountSplitButton, createSingleFlight } from "../core/splitButton.js";
import { confirmAction } from "../core/confirm.js";
import { refreshFeed } from "../core/taskFeed.js";
import { SMALLEST_THREAD_PAGE } from "../core/thread.js";
import {
  branchCloseout,
  branchFinishConfirm,
  branchFinishFacts,
  branchFinishFailureSummary,
  branchInboxKey,
} from "../core/branchFinish.js";
import { isPending, removeRecord, runOptimistic } from "../core/optimistic.js";
import "../styles/shell.css";
import "../styles/surfaces.css";

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

/** Background adoption may change a Files pane's backing while it owns a
 * live draft. The next poll can remount after that draft is saved. */
export function shouldRetainDirtyFilesPane(tab, pane) {
  return tab === "files" && Boolean(pane?.hasUnsavedChanges?.());
}

const paneKey = (tab, scope) => `${tab}:${reviewKeyOf(scope) || (scope ? "primary" : "none")}`;

/** Where the Files tab is standing: the URL says, so a sent link opens the same
 *  file and a reload keeps the reader's place. Pure. */
const openPlaceOf = (route) => (route.file ? { path: route.file, line: route.line || null } : null);

function mountPlainChanges(host, onInitialize) {
  App.routeLeaveGuard = null;
  host.innerHTML = `<div class="pane-split changes2"><aside class="crail crail-host"></aside><main class="empty folder-git-empty"><h2>Initialize Git</h2><p>Track changes and create branches in this folder.</p><button class="btn primary" id="init-git" type="button">Initialize Git</button><p class="error" id="init-git-status" role="status"></p></main></div>`;
  host.querySelector("#init-git").onclick = onInitialize;
}

export async function renderBranch() {
  const root = $("#root");
  const { deviceId, projectId, branch } = App.route;
  // The machine this link is about, read once: everything mounted below is
  // handed its caller, its cache scope, its conversations and its offline mark
  // from here, so no pane has to ask which device it is on.
  const context = routeContext(App.route);
  // The account's name for this project — the pair (device, project), since
  // every machine mints a `p1`.
  const projectKey = routeProjectKey(App.route);
  let tab = App.route.tab || "changes";
  // Consumed once: only the navigation the toolbar's create form just fired
  // means it, and a later revisit to this same branch must not keep stealing
  // focus back to the composer.
  const autofocusComposer = App.focusComposerOnMount;
  App.focusComposerOnMount = false;
  const openAt = openPlaceOf(App.route);
  root.className = "surface";
  // A machine that cannot answer — never opened here, or gone since — has
  // nothing under this link to read or write, so the surface names it rather
  // than standing a frame up over calls that can only be refused. The notice
  // waits for that machine and hands the link back when it lands.
  if (!canAnswer(context)) {
    mountDeviceNotice(root, deviceId);
    return;
  }
  root.innerHTML = `<div id="tabbody" class="flush"><div class="empty">loading…</div></div>`;
  /** Changes/Files live in the shell's own icon rail between the inbox and the
   *  work (core/directoryRail.js) — the same rail a workspace directory uses,
   *  because they are the same two faces of one checkout. It is part of the
   *  shell, so it remains available while the pane is loading or has no
   *  checkout. */
  const paintTabs = () => {
    const bar = $("#dir-rail");
    if (!bar) return;
    paintDirectoryRail(bar, {
      active: tab,
      onSelect: (next) => go({ name: "branch", deviceId, projectId, branch, tab: next }),
    });
  };
  paintTabs();
  // The basement, at the bottom of the view column: this branch's checkout, as
  // terminals. Shut unless the last visit left it open.
  let consolePanel = null;
  // The agents beside the work, not instead of it: the rail belongs to this
  // branch, so it is mounted with the surface and torn down with it. Which
  // bubble is open is the whole surface's business — the row this view reads
  // carries that agent's conversation, and the review comments Changes sends go
  // into it — so the choice lives in a handle they share.
  const agentSelection = createAgentSelection();

  let disposed = false;
  let row = null; // the branch.get payload: the feed row plus `run`
  // This machine answers now. If it goes while the surface is open, what was
  // read stays on screen and the strip says whose state that is — but only once
  // there is something to be whose: until the row lands this frame says
  // "loading…", and nothing on it came from that machine at all.
  const deviceStrip = mountDeviceStrip(root, context, { hasContent: () => Boolean(row) });
  let pane = null; // the mounted tab body ({ dispose })
  let mountedKey = null; // what the body was mounted over: tab + review key
  let reviewPlug = null; // ONE instance per backing, so pending comments survive
  let reviewKey = null;

  // How everything below asks this machine. It is the device's caller, not the
  // session's: the machine can drop and resume under a mounted surface, and the
  // surface goes on asking the machine rather than the socket it was built
  // over. An operation already awaiting a reply still finishes on the session
  // that accepted it.
  const callRpc = context.rpc;
  // The frozen treatment every pane shows while its machine is unreachable: one
  // device going offline says nothing about the others.
  const isOffline = () => context.offline;
  // Two surfaces here can mutate an unclaimed checkout first — the rail's first
  // message and the review's first comment or action — and near-simultaneous
  // adoptions would ask for two owners of one checkout. Both take their adopter
  // from here, so the checkout is claimed once.
  const adopterFor = createAdopters(callRpc);
  const adoptingHere = () => adopterFor(branchScope(row, projectId));

  let rail = null;
  // The work item and the machine it is on — the address the console and the
  // rail are both mounted at, minted once so the two cannot drift apart.
  const workAddress = {
    kind: "branch",
    deviceId,
    projectId,
    branch,
    call: callRpc,
    cacheScope: context.cacheScope,
  };
  const ensureBranchChrome = () => {
    if (!consolePanel) consolePanel = mountConsole($("#console-region"), { ...workAddress });
    if (!rail)
      rail = mountAgentRail($("#agent-rail"), {
        ...workAddress,
        selection: agentSelection,
        adopting: adoptingHere,
        autofocusComposer,
        chatRepository: context.chatRepository,
      });
  };
  const home = () => go({ name: "inbox" });
  /** An ending the user triggered here must not badge its own inbox entry:
   *  Merged/Abandoned are attention-class, so the entry's cursor is cleared on
   *  the way out (the Stage B rule; core/inboxView.js noteSelfAction). An issue
   *  handed BACK to the inbox keeps its own cursor — it is asking for somebody
   *  again, and the event naming the branch it lost is the point of it. Only an
   *  issue that ends with the branch is cleared with it. */
  const finished = ({ issueEnded = false } = {}) => {
    noteSelfAction(entityIdOf(row), issueEnded ? row && row.issue_id : null);
    home();
  };

  // ---- the way the branch ends ------------------------------------------------
  //
  // ONE latch for the surface's Done: the row poll repaints this control, and a
  // repaint mid-flight would arm a second branch.finish over the first.
  const finishFlight = createSingleFlight();

  /** One close-out: read what the deletion costs off the freshest row, confirm
   *  the exact outline, send it, and leave for the inbox. */
  const runFinish = async (optionId) => {
    const facts = branchFinishFacts(row, branch);
    const name = facts.branch;
    // A cancel throws BEFORE any RPC: the button restores and no notice appears.
    if (!(await confirmAction(branchFinishConfirm(facts)))) throw new Error("cancelled");
    const inboxKey = branchInboxKey(row, { projectId, branch: name, projectKey });
    if (isPending(INBOX_SCOPE, inboxKey)) return;
    const finishing = runOptimistic({
      scope: INBOX_SCOPE,
      records: [removeRecord(inboxKey)],
      call: () =>
        finishWorkItem(
          {
            kind: "branch",
            entityId: entityIdOf(row),
            issueId: row && row.issue_id,
            // Which machine the branch is on: the link said, and the verb goes
            // to that device.
            deviceId,
            projectId,
            branch: name,
            // The issue ends with the branch only when the work landed;
            // otherwise the bridge hands it back to the inbox.
            issueEnded: facts.merged,
          },
          optionId,
        ),
      failureSummary: branchFinishFailureSummary(name),
    });
    home();
    await finishing;
    await refreshFeed();
  };

  // What the Done control was last painted from. The row poll runs every 1.6
  // seconds and almost every tick resolves the same close-out; rewriting the
  // host on each one destroyed whatever was open inside it, so the menu
  // vanished before the user could reach an item.
  let paintedFinish = null;

  /** Paint the branch's Done into the toolbar's verb slot, off the freshest
   *  branch.get row. Frozen while a close-out is in flight, so no poll — this
   *  view's own row poll, or the toolbar's independent one, which also calls
   *  this via setToolbarVerb below — can remount an enabled button over a
   *  pending branch.finish, and while its menu is open — a click in progress
   *  outranks a repaint, which lands on a later tick once the menu is shut. */
  const paintFinish = (host) => {
    host = host || $("#tb-verb");
    if (!host || finishFlight.active()) return;
    if (host.querySelector(".splitmenu:not([hidden])")) return;
    const closeout = branchCloseout(row);
    const signature = JSON.stringify(closeout);
    if (signature === paintedFinish) return;
    paintedFinish = signature;
    if (!closeout.shown) {
      host.innerHTML = "";
      return;
    }
    // Sized down to the toolbar's own vocabulary — this is a fact in a bar of
    // facts, not the loudest thing on the page — but always pressable: Done is
    // never refused for the state of the work, what the deletion would cost is
    // in the confirmation, not in a disabled button.
    mountSplitButton(host, { options: closeout.options, run: runFinish, flight: finishFlight, variant: "mini" });
  };
  setToolbarVerb(paintFinish);

  const navigate = {
    openFile: ({ path, line }) => go({ name: "branch", deviceId, projectId, branch, tab: "files", file: path, line }),
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
          navigate,
          getTask: () => (row ? row.run : null),
          isOffline,
          cacheScope: context.cacheScope,
          agentSelection,
          viewingContext: App.viewingContext,
          // A merge is the work landing: the issue it implements ends with it.
          onMerged: () => finished({ issueEnded: true }),
        });
      } else {
        reviewPlug = createWorktreeReview({
          projectId: scope.project_id,
          worktreeId: scope.worktree_id,
          callRpc,
          navigate,
          adopting: adopterFor(scope),
          isOffline,
          cacheScope: context.cacheScope,
          viewingContext: App.viewingContext,
          // Adoption keeps the URL — the same branch now stands on a run, so
          // the surface re-resolves and the Changes rail re-mounts run-backed.
          onAdopted: () => refresh(true),
          onFinished: () => finished(),
          onGone: () => refresh(true),
        });
      }
    }
    const plug = reviewPlug;
    // Spread, never a hand-written subset. This wrapper exists to answer ONE
    // question the plug cannot — which branch a run's diff is against — and an
    // adapter that re-declares the rest silently drops whatever the plug learns
    // to do next. It did: `mount(host)` swallowed the options the pane passes,
    // so the merge verb had no host and `commentOffer` did not exist, which
    // took the whole toolbar down with it.
    return {
      ...plug,
      getBase: () => (scope.run_id ? (row && row.run && row.run.base_branch) || "main" : plug.getBase()),
    };
  };

  const mountFreshBody = (host, scope) => {
    if (!scope) {
      host.innerHTML = `<div class="empty">No checkout on this device carries <span class="mono">${esc(branch)}</span>.</div>`;
      return;
    }
    if (tab === "files") {
      pane = renderFilesTab(host, {
        scope,
        callRpc,
        cacheScope: context.cacheScope,
        openAt,
        viewingContext: App.viewingContext,
        // Moving within the tab: the URL keeps up without the surface being
        // rebuilt around the file it is already showing.
        onFileOpen: (path) => markRoute({ name: "branch", deviceId, projectId, branch, tab: "files", file: path }),
      });
      App.routeLeaveGuard = pane.canLeave;
      return;
    }
    if (row && row.is_git === false) {
      mountPlainChanges(host, async (event) => {
        const button = event.currentTarget;
        button.disabled = true;
        try {
          await callRpc("project.init_git", { project_id: projectId });
          if (disposed) return;
          await refreshFeed();
          if (disposed) return;
          row = null;
          mountedKey = null;
          await refresh(true);
          if (disposed) return;
          ensureBranchChrome();
        } catch (error) {
          if (disposed) return;
          button.disabled = false;
          host.querySelector("#init-git-status").textContent = error.message || String(error);
        }
      });
      return;
    }
    App.routeLeaveGuard = null;
    pane = mountGitPane(host, {
      scope,
      callRpc,
      cacheScope: context.cacheScope,
      agentCommitOptions: row && row.run ? taskAgentCommitOptions(row.run.state, row.run.goal) : [],
      review: reviewFor(scope),
      agentSelection,
      navigate,
      viewingContext: App.viewingContext,
      // Review prioritization: the run's freshest triage pass orders whichever
      // changeset is open, and the reviewer's trust dial is remembered for the
      // project they are reading.
      projectId,
      triageEnabled: () => Boolean(row && row.run && row.run.triage_enabled === true),
      triage: () => (row && row.run && row.run.triage) || null,
    });
  };

  /** Mount the open tab's body over the resolved row. Idempotent per
   *  (tab, backing): polls repaint nothing — the panes own their own polls —
   *  so only a change of backing (adoption, worktree pruned) remounts. */
  const mountBody = () => {
    const host = $("#tabbody");
    if (!host) return;
    const scope = branchScope(row, projectId);
    const key = paneKey(tab, scope);
    if (key === mountedKey) return;
    // Adoption can change the backing key under this same Files surface. Keep
    // its live editor mounted until the draft is saved or explicitly left;
    // polling must never turn a background ownership update into data loss.
    if (shouldRetainDirtyFilesPane(tab, pane)) {
      pane.retargetScope(scope);
      mountedKey = key;
      return;
    }
    pane?.dispose();
    pane = null;
    mountedKey = key;
    mountFreshBody(host, scope);
  };

  /** One read of the branch row. `force` remounts even when the backing is
   *  unchanged (an adoption just happened underneath the plug). */
  // eslint-disable-next-line complexity -- ratchet: this callback is at 12, cap 10 — reduce it, then drop this line
  const refreshGit = async (force = false) => {
    let payload;
    try {
      payload = await callRpc("branch.get", {
        project_id: projectId,
        branch,
        ...agentSelection.scope(),
        ...SMALLEST_THREAD_PAGE,
      });
    } catch {
      // The branch stopped resolving: merged away, renamed, or the worktree is
      // gone. A row we already painted stays; a first read that fails says so —
      // once. Every tick after says the same thing, and repainting would rebuild
      // the one way out the empty state offers.
      if (!disposed && !row && mountedKey !== "gone") {
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
    const runAppeared = Boolean(payload.run) !== Boolean(row && row.run);
    row = payload;
    // A remount when the run's knowledge appears (the feed-seeded row carries
    // ids but not the run body), so the commit box gets its agent options.
    if (force || runAppeared) mountedKey = null;
    mountBody();
    paintFinish();
    ensureBranchChrome();
  };

  const refresh = async (force = false) => {
    if (row && row.is_git === false) {
      const gitState = await projectGitState(callRpc, projectId);
      if (disposed) return;
      if (gitState !== true) {
        if (force) mountedKey = null;
        mountBody();
        return;
      }
      row = null;
      mountedKey = null;
    }
    await refreshGit(force);
  };

  let watcher = null;
  App.viewDispose = () => {
    disposed = true;
    // The view ends its own read rather than trusting the shell to clear the
    // slot it put it in.
    if (watcher) watcher.dispose();
    watcher = null;
    const dirRail = $("#dir-rail");
    if (dirRail) dirRail.innerHTML = "";
    clearToolbarVerb(paintFinish);
    if (pane) pane.dispose();
    pane = null;
    rail?.dispose();
    consolePanel?.dispose();
    deviceStrip();
  };
  // The feed already carries this branch's row — ids, scope, agents — and the
  // cached snapshot replays synchronously at subscribe. Standing the tabs and
  // panes up from it means switching branches shows the full surface (which
  // then fills from its own caches) instead of a bare loading frame for the
  // length of a round trip; the first live read reconciles.
  if (!row) {
    const initial = await initialBranchState(callRpc, { deviceId, projectId, branch, requestedTab: App.route.tab });
    if (disposed) return;
    row = initial.row;
    tab = initial.tab;
    paintTabs();
    if (row) {
      mountBody();
      paintFinish();
    }
  }
  if (!row || row.is_git !== false) {
    ensureBranchChrome();
  }
  await refresh();
  // The first read can outlive the view: a navigation mid-flight has already
  // torn this view down (render() ran viewDispose), and the poll slot belongs
  // to whatever is mounted now. Claiming it here would orphan an interval that
  // reads a dead branch forever — the leaked-poller slowdown.
  if (disposed) return;
  // The run behind the branch is the entity whose events say this row moved;
  // until the first read names one (an unadopted checkout has none), the safety
  // poll is what carries the surface.
  watcher = watchChanges({
    refresh,
    intervalMs: ROW_POLL_MS,
    entity: () => [row && row.run_id, row && row.worktree_id],
    // Focus tier: the mounted work surface reads all four kinds of this
    // checkout, and wants them as they happen.
    kinds: ["state", "thread", "git", "files"],
    mode: "realtime",
  });
  App.poll = watcher;
}
