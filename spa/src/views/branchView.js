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
// The surface resolves what stands under the branch with `branch.get`: a run
// or a bare worktree. That resolution names the git scope the tab bodies read,
// and which review plug the Changes rail carries — a run's aggregate review
// diff (taskReview) or a bare worktree's adopt-on-comment diff
// (worktreeReview). A plain folder has neither: it is the project's own
// directory, and it browses files with no review entry at all.
//
// A branch name comes from the repo: untrusted, and escaped everywhere it is
// painted.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go, markRoute } from "../app.js";
import { deviceFeedNow } from "../core/feedRows.js";
import { paintDirectoryRail } from "../core/directoryRail.js";
import { setToolbarVerb, clearToolbarVerb } from "../core/toolbar.js";
import { dropShell, refineShell, shellSelection } from "../core/shell.js";
import { mountGitPane, taskAgentCommitOptions } from "../core/gitPane.js";
import { renderFilesTab } from "./files.js";
import { createTaskReview } from "./taskReview.js";
import { createWorktreeReview } from "./worktreeReview.js";
import { branchStateIn, initialBranchState, projectGitState } from "./branchSeed.js";
import { createAdopters } from "../core/adoption.js";
import { INBOX_SCOPE, finishWorkItem, noteSelfAction } from "../core/inboxView.js";
import { entityIdOf } from "../core/entityId.js";
import { surfaceContext } from "../core/surfaceContext.js";
import { routeProjectKey } from "../core/deviceKey.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { mountSplitButton, createSingleFlight } from "../core/splitButton.js";
import { confirmAction } from "../core/confirm.js";
import { refreshFeed, subscribeFeed } from "../core/taskFeed.js";
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

/** The git scope of what stands under the branch row: exactly one of
 *  { run_id } / { project_id, worktree_id } / { project_id }, or null when
 *  there is no row at all and no project to fall back on.
 *
 *  A row that names neither a run nor a worktree is the project's own
 *  directory — the repository the branch is checked out in, or a plain folder
 *  with no git in it — and the project alone names that. Pure. */
export function branchScope(row, projectId) {
  if (!row) return null;
  if (row.run_id) return { run_id: row.run_id };
  const project = row.project_id || projectId;
  if (!project) return null;
  if (row.worktree_id) return { project_id: project, worktree_id: row.worktree_id };
  return { project_id: project };
}

/** What the Changes rail's review plug is made for — a plug survives repaints
 *  only while this key holds, so pending comments outlive tab switches but
 *  never leak across an adoption (worktree → run). Pure. */
export function reviewKeyOf(scope) {
  if (!scope) return null;
  if (scope.run_id) return `run:${scope.run_id}`;
  if (scope.worktree_id) return `worktree:${scope.worktree_id}`;
  return null; // a plain folder has no aggregate review entry
}

/** Background adoption may change a Files pane's backing while it owns a
 * live draft. The next poll can remount after that draft is saved. */
export function shouldRetainDirtyFilesPane(tab, pane) {
  return tab === "files" && Boolean(pane?.hasUnsavedChanges?.());
}

const paneKey = (tab, scope) => `${tab}:${reviewKeyOf(scope) || (scope ? "folder" : "none")}`;

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
  const context = surfaceContext(App.route);
  // The account's name for this project — the pair (device, project), since
  // every machine mints a `p1`.
  const projectKey = routeProjectKey(App.route);
  let tab = App.route.tab || "changes";
  // Consumed once: only the navigation the toolbar's create form just fired
  // means it, and a later revisit to this same branch must not keep stealing
  // focus back to the composer.
  const openAt = openPlaceOf(App.route);
  root.className = "surface";
  // The surface paints what the records hold of this machine whether or not it
  // can answer. Only a machine nothing here has ever held has nothing to paint:
  // the notice names it, waits for it, and hands the link back when it lands.
  if (!context) {
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
  // Which bubble is open is the whole surface's business — the row this view
  // reads carries that agent's conversation, and the review comments Changes
  // sends go into it — so the choice lives in a handle they share. The rail
  // itself is the shell's, so the handle is taken from there rather than minted
  // here: a page that minted its own would be talking to a rail that never
  // heard of it.
  const agentSelection = shellSelection();

  let disposed = false;
  let row = null; // the cached feed row, with the run's own body on it
  // While the machine cannot answer, what the records hold stays on screen and
  // the strip says whose state that is — but only once there is something to
  // be whose: until the row lands this frame says "loading…", and nothing on it
  // came from that machine at all.
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

  // The rail and the console beside this page are the shell's (core/shell.js),
  // standing on the branch the route names. The one fact the shell cannot know
  // is who claims the checkout under them: this page holds the single adopter
  // the rail's first message and the review's first comment both take theirs
  // from, so it is handed over rather than built a second time.
  const ensureBranchChrome = () => refineShell({ adopting: adoptingHere });
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
   *  the first comment or action; a plain folder carries none. */
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

  /** The branch has nothing under it on this machine: merged away, renamed, or
   *  the checkout is gone. Said once — every later delivery says the same
   *  thing, and repainting would rebuild the one way out this offers. */
  const paintGone = () => {
    if (row || mountedKey === "gone") return;
    const host = $("#tabbody");
    if (!host) return;
    host.innerHTML = `<div class="empty gone">No checkout in this project carries <span class="mono">${esc(branch)}</span>.<div><button class="btn" id="branchback">Back to inbox</button></div></div>`;
    const back = $("#branchback");
    if (back) back.onclick = () => home();
    mountedKey = "gone";
  };

  /** Take up a row the cache answered with. `force` remounts even when the
   *  backing is unchanged (an adoption just happened underneath the plug). */
  const takeRow = (next, force) => {
    if (!next) {
      paintGone();
      return;
    }
    // A remount when the run's knowledge appears — a checkout adopted under
    // the surface gains one — so the commit box gets its agent options.
    const runAppeared = Boolean(next.run) !== Boolean(row && row.run);
    row = next;
    if (force || runAppeared) mountedKey = null;
    mountBody();
    paintFinish();
    ensureBranchChrome();
  };

  /** What this machine's slice says the branch is right now. Null while the
   *  cache holds nothing at all for the machine — a deep link that landed
   *  before its first pass — which is not the same as "no such branch". */
  const rowNow = () => {
    const snapshot = deviceFeedNow(deviceId);
    return snapshot ? { held: true, row: branchStateIn(snapshot, projectId, branch).row } : { held: false, row: null };
  };

  /** What a delivery has to change before the surface reads it again.
   *
   *  The row alone is not enough. A machine with no records answers no row and
   *  so does a machine that has answered and carries no such branch — and the
   *  step between those two IS the empty state, the one paint a cold mount is
   *  otherwise never given. */
  const deliverySignature = (answer) => JSON.stringify([answer.held, answer.row]);

  const refresh = (force = false) => {
    if (disposed) return;
    if (row && row.is_git === false) {
      // A folder that has since been initialized is a repository, and its row
      // is the board's rather than this stand-in.
      if (projectGitState(deviceId, projectId) !== true) {
        if (force) mountedKey = null;
        mountBody();
        return;
      }
      row = null;
      mountedKey = null;
    }
    const answer = rowNow();
    if (!answer.held && !answer.row) return;
    takeRow(answer.row, force);
  };

  let unwatch = null;
  App.viewDispose = () => {
    disposed = true;
    // The view stops hearing the cache rather than trusting the shell to clear
    // the slot it put it in.
    if (unwatch) unwatch();
    unwatch = null;
    const dirRail = $("#dir-rail");
    if (dirRail) dirRail.innerHTML = "";
    clearToolbarVerb(paintFinish);
    if (pane) pane.dispose();
    pane = null;
    deviceStrip();
  };
  // The cache already carries this branch's row — ids, scope, agents, and the
  // run's own body — and the feed replays its snapshot synchronously at
  // subscribe. So the whole surface is up on the first frame: switching
  // branches never shows a bare loading frame for the length of a round trip.
  if (!row) {
    const initial = initialBranchState({ deviceId, projectId, branch, requestedTab: App.route.tab });
    row = initial.row;
    tab = initial.tab;
    paintTabs();
    if (row) {
      mountBody();
      paintFinish();
    }
  }
  // A folder that is not a repository yet has no conversation to hold and no
  // checkout to open terminals in, so the shell's parts come down rather than
  // standing empty over the Initialize Git offer.
  if (!row || row.is_git !== false) ensureBranchChrome();
  else dropShell();
  refresh();
  if (disposed) return;
  // A deep link can land before this machine has any records — a reload, a
  // link opened in a new tab. Ask for the pass that settles whether the branch
  // is there at all; what it writes comes back as a delivery, the same way a
  // push does. views/workspaceView.js asks the same question of a workspace.
  if (!rowNow().held) void refreshFeed(deviceId);
  // Every later frame is the cache's: a `state` push rewrites this row's
  // record, the feed re-reads it, and the delivery is what repaints. There is
  // nothing behind this to poll.
  let painted = deliverySignature(rowNow());
  unwatch = subscribeFeed(() => {
    if (disposed) return;
    const answer = rowNow();
    const signature = deliverySignature(answer);
    if (signature === painted) return;
    painted = signature;
    refresh();
  });
  App.poll = { dispose: () => unwatch && unwatch() };
}
