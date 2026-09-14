// DOM-light controller for the Changes surface: a left rail (— where a view
// plugs one in — the review aggregate at the top, Uncommitted under it with its
// +/− counts, and the commit list under both) driving a right detail pane. Every
// changeset the pane can show renders through ONE renderer: stacked full file
// diffs with line numbers, a per-file header carrying counts, a ✎, and a ⋯ for
// the file's own verbs. There is no staging and no changed-files list — commit
// is commit-all, and per-file discard lives in the ⋯.
//
// Owns a 1.6s git.status + git.log poll with a keyed freeze; the freeze also
// holds while the reviewer is mid-comment, has a menu open, or has a git action
// in flight. The pure helpers (poll key, option lists) are exported for unit
// tests; mountGitPane is the only DOM-touching entry point.

import { esc } from "./text.js";
import { directoryCacheId } from "./directoryScope.js";
import { gitToolbarHtml, AGENT_COMMIT_MESSAGE } from "./gitRender.js";
import {
  changesRailEntries,
  uncommittedHeaderHtml,
  commitHeaderHtml,
  changesetPlaceholderHtml,
} from "./changesRender.js";
import {
  defaultChangesSelection,
  selectionAfterPoll,
  commitBoxVisible,
  commitAllPaths,
  commentsSupported,
  hasUncommittedChanges,
} from "./changesModel.js";
import { createAgentSelection } from "./agentSelection.js";
import { createCommentLayer } from "./changesComments.js";
import { changedSinceChangeset, stampChangeset } from "./reviewMemory.js";
import { loadTrustDial, saveTrustDial, triageFingerprint } from "./triageModel.js";
import { createTriageOverrides } from "./triageOverride.js";
import { createFileFolds, fileKey, pathOf } from "./diff.js";
import { fileFoldOf, stackClaims } from "./diffRender.js";
import { fileStackEntries, fileViewFromStatus } from "./fileEntries.js";
import { watchEditedTimes } from "./editedTime.js";
import { createFileDiffs, wholePatch } from "./fileDiffs.js";
import { timedPaint } from "./paintTiming.js";
import { DIFF_PLACE_KEEPING, createChangesetPaint } from "./diffPlace.js";
import { initPaneDrawer, paneDrawerHtml } from "./paneDrawer.js";
import { mountSplitButton } from "./splitButton.js";
import { mountChangesComposer } from "./changesComposer.js";
import { commitPaths, createReviewMarks } from "./reviewMarks.js";
import { toggleSecretSpoiler } from "./secrets.js";
import { watchChanges } from "./changeEvents.js";
import { readCached, writeCached } from "./localCache.js";
import { patchList } from "./patchList.js";
import { paintKeepingPlace } from "./paintKeepingPlace.js";
import { MUTATION_THREAD_PAGE } from "./thread.js";
import { el } from "../dom.js";
import { createParsedDiffCache } from "./parsedDiffCache.js";
import { createDiffViewport } from "./diffViewport.js";
import { createReviewPlug } from "./changesReview.js";

export const GIT_PANE_POLL_MS = 1600;

/** Workspace directories review everything not represented by their push
 * destination. The plug is created here so every workspace Git pane gets the
 * aggregate without each hosting view having to remember special wiring. */
export function createWorkspaceReview({ scope, callRpc, navigate = null, viewingContext = null, onBaseChange = () => {} }) {
  let base = { kind: "empty", label: null };
  const plug = createReviewPlug({
    navigate,
    viewingContext,
    entity: scope.workspace_id,
    cacheEntity: directoryCacheId(scope),
    fetchDiff: async (ifDiffKey) => {
      const payload = await callRpc("git.unpushed", {
        ...scope,
        ...(ifDiffKey ? { if_diff_key: ifDiffKey } : {}),
      });
      if (!payload.unchanged && payload.base) {
        const changed = payload.base.kind !== base.kind || payload.base.label !== base.label;
        base = payload.base;
        if (changed) onBaseChange();
      }
      return { ...payload, commentable: false };
    },
  });
  return {
    ...plug,
    getBase: () => base.label || (base.kind === "published_ancestor" ? "published history" : "Not pushed yet"),
    getRailSubtitle: () =>
      base.label ? `vs ${base.label}` : base.kind === "published_ancestor" ? "since published history" : "Not pushed yet",
  };
}

// ---- repo-management decision helpers (v2) -----------------------------
// Pure, exported, and load-bearing in the controller below. Every one tolerates
// the additive git.status fields being ABSENT on an older bridge.

/** The v2 repo-management surface (toolbar/banner/discard) only exists once the
 *  bridge reports repo_state — an older bridge omits it, and every new control
 *  degrades to hidden rather than rendering NaN/undefined text. */
export function supportsRepoManagement(status) {
  return Boolean(status && typeof status.repo_state === "string");
}

/** The ahead/behind chip data, or null when there is nothing meaningful to show:
 *  an older bridge (no repo_state), a detached/upstream-less branch (null
 *  upstream), or non-numeric counts. */
export function syncChipState(status) {
  if (!supportsRepoManagement(status)) return null;
  if (typeof status.upstream !== "string" || !status.upstream) return null;
  const ahead = Number(status.ahead);
  const behind = Number(status.behind);
  if (!Number.isFinite(ahead) || !Number.isFinite(behind)) return null;
  return { ahead, behind };
}

/** Every toolbar verb is disabled while any action RPC is in flight (the same
 *  freeze that suppresses poll repaints), so a mid-action repaint never revives
 *  a live button to double-fire. */
export function toolbarControlsDisabled(inFlightCount) {
  return inFlightCount > 0;
}

/** The re-enable decision for the shared settle path: only once the LAST
 *  in-flight action settles do the controls come back (a nested/overlapping
 *  action must not revive a live button early). */
export function actionSettleReenables(inFlightCount) {
  return inFlightCount === 0;
}

/** The button vocabulary the git bar's verbs wear. The bar is a dense toolbar
 *  of secondary actions, so Fetch and the Pull/Push/Stash split buttons are all
 *  the app's mini button — no look of its own. */
export const TOOLBAR_BUTTON_VARIANT = "mini";

/** The controls one settled action re-enables — every toolbar verb PLUS the box
 *  under the diff. This is the single source S1 unifies on: a toolbar action's
 *  repaint disables that box's button (render() disables it while any action is
 *  in flight), so the SAME settle that re-enables the toolbar must also re-enable
 *  it, or a Fetch/Push leaves it stuck disabled. */
export function settleReenableSelectors() {
  return [".gtfetch", ".gtsync .btn", ".gtstash .btn", ".csbox-actions .btn:not(.caret)"];
}

/** Pull split button: fast-forward primary, then merge / rebase in the menu. */
export function pullSplitOptions() {
  return [
    { id: "pull", label: "Pull", description: "fast-forward only", busyLabel: "Pulling…" },
    { id: "pull_merge", menuLabel: "Pull (merge)", description: "merge the upstream changes", busyLabel: "Pulling…" },
    { id: "pull_rebase", menuLabel: "Pull (rebase)", description: "rebase onto the upstream", busyLabel: "Pulling…" },
  ];
}

/** Push split button: plain push, then a danger-styled force-push-with-lease
 *  (never a bare --force) gated behind an inline confirm in the controller. When
 *  `armed`, the force-push item reads "Confirm force push?" so the pending
 *  two-click confirm is VISIBLE in the menu (S2a) — not just a transient hint. */
export function pushSplitOptions(armed = false) {
  return [
    { id: "push", label: "Push", description: "push to the upstream", busyLabel: "Pushing…" },
    {
      id: "force_push",
      menuLabel: armed ? "Confirm force push?" : "Force push (with lease)",
      description: "overwrite remote history — safely",
      busyLabel: "Force pushing…",
      danger: true,
    },
  ];
}

/** Stash split button: stash everything (incl. untracked), then pop — the pop
 *  option carries the stash count as a badge only when there is a stash. */
export function stashSplitOptions(stashCount = 0) {
  const count = Number(stashCount) || 0;
  return [
    { id: "stash", label: "Stash", description: "stash all changes, including untracked", busyLabel: "Stashing…" },
    { id: "stash_pop", menuLabel: count > 0 ? `Pop stash (${count})` : "Pop stash", description: "apply and drop the latest stash", busyLabel: "Popping…" },
  ];
}

// The one place option ids become wire calls. Fresh params per call so a caller
// can never mutate the shared template.
const SYNC_RPC = {
  fetch: { method: "git.fetch", params: {} },
  pull: { method: "git.pull", params: {} },
  pull_merge: { method: "git.pull", params: { mode: "merge" } },
  pull_rebase: { method: "git.pull", params: { mode: "rebase" } },
  push: { method: "git.push", params: {} },
  force_push: { method: "git.push", params: { force: true } },
  stash: { method: "git.stash", params: {} },
  stash_pop: { method: "git.stash_pop", params: {} },
};

/** Map a sync/stash option id onto its RPC { method, params }. Throws on an
 *  unknown id (fail fast — a typo must not silently no-op). */
export function syncActionRpc(optionId) {
  const entry = SYNC_RPC[optionId];
  if (!entry) throw new Error(`unknown sync action ${optionId}`);
  return { method: entry.method, params: { ...entry.params } };
}

/** The inline two-click confirm machine shared by every destructive verb
 *  (discard, force push, merge abort): a first touch arms the control (returns
 *  its key as the new pending); a second touch of the SAME control fires and
 *  disarms; touching a different control re-arms that one. */
export function resolveInlineConfirm(pending, key) {
  if (pending === key) return { fire: true, pending: null };
  return { fire: false, pending: key };
}

/** How long an armed inline confirm stays live before the poll auto-disarms it.
 *  A destructive verb (force push / discard / abort) armed and then abandoned
 *  must not stay one click from firing indefinitely. */
export const INLINE_CONFIRM_TTL_MS = 10000;

/** True when an armed confirm (stamped at `armedAt`) has aged past the TTL, so
 *  the poll should disarm it and repaint. `armedAt` null/undefined → nothing is
 *  armed → never expired. */
export function confirmExpired(armedAt, now, ttlMs = INLINE_CONFIRM_TTL_MS) {
  if (armedAt === null || armedAt === undefined) return false;
  return now - armedAt >= ttlMs;
}

// The destructive sync-menu options that require the inline confirm before they
// fire (the primary Pull/Push/Stash/Pop actions fire immediately).
const SYNC_CONFIRM_OPTIONS = new Set(["force_push"]);

/** True when a sync option must be confirmed once before it runs. */
export function syncActionNeedsConfirm(optionId) {
  return SYNC_CONFIRM_OPTIONS.has(optionId);
}

// The operation label for every abortable in-progress state (each maps to a
// state-appropriate `git … --abort` on the bridge). Kept as the single copy of
// the noun so the banner reads "<Op> in progress …" without a per-state string.
const REPO_STATE_OP_LABEL = {
  merging: "Merge",
  rebasing: "Rebase",
  "cherry-picking": "Cherry-pick",
  reverting: "Revert",
  bisecting: "Bisect",
};

/** The state banner decision for a non-clean repo, or null when clean/absent.
 *  This is the SINGLE source of the banner copy + `abortable` flag; gitRender's
 *  gitStateBannerHtml only renders what this returns. `abortable` gates the
 *  Abort button: only the in-progress ops (merge/rebase/cherry-pick/revert/
 *  bisect) can be aborted — merge_abort rejects "conflicted"/"other"/"clean",
 *  so offering Abort there would only produce errors. */
export function repoStateBanner(repoState) {
  const op = REPO_STATE_OP_LABEL[repoState];
  if (op) return { message: `${op} in progress — resolve conflicts, then continue.`, abortable: true };
  if (repoState === "conflicted")
    return { message: "Conflicts in the working tree — resolve them, then commit (or discard the files).", abortable: false };
  if (repoState === "other") return { message: "Repository is in an unusual state.", abortable: false };
  return null;
}

/** The repaint-freeze key for one poll's payloads: the bridge's own status_key
 *  — a hash over everything a repaint depends on, branch and HEAD and repo state
 *  and every file's stage state and content key — plus the visible commit page.
 *  Unchanged key → the poll leaves the DOM (and the user's caret) alone.
 *
 *  The status carries no patch any more: a file's body is fetched on its own and
 *  keyed by its content key, so what moved in the working tree reaches the key
 *  through the shape rather than through a megabyte of diff. */
export function gitPollKey(status, log, nowSeconds = Date.now() / 1000) {
  const commits = ((log && log.commits) || []).map((c) => c.hash).join(",");
  // A coarse minute bucket: relative commit ages re-render at most once a
  // minute even when the repo itself is untouched.
  const minuteBucket = Math.floor(nowSeconds / 60);
  return [status.status_key, commits, Boolean(log && log.more), minuteBucket].join("\x03");
}

/** What to ask git.status with: the key the pane already holds, so a repo that
 *  has not moved answers `{ unchanged: true }` and the bridge never renders a
 *  diff nobody asked for. A pane holding no status asks for the whole shape. */
export function ifStatusKey(status) {
  return status && status.status_key ? { if_status_key: status.status_key } : {};
}

/** The status the pane holds after one poll: the shape it was handed, or the
 *  one it already had when the bridge says the key it was sent still stands. */
export function statusAfterPoll(answer, held) {
  return answer && answer.unchanged ? held : answer;
}

/** The stash key for a scope's in-progress commit-message draft: drafts live in
 *  a module-level Map so tab switches and view-shell rebuilds (which remount the
 *  pane from scratch) restore them transparently. Keyed on the narrowest id the
 *  scope carries — a worktree scope names its project too, and keying on that
 *  would pool every worktree's draft with the project's own. */
export function gitDraftKey(scope) {
  if (scope.workspace_id) return directoryCacheId(scope);
  if (scope.run_id) return `run:${scope.run_id}`;
  if (scope.worktree_id) return `worktree:${scope.worktree_id}`;
  return `project:${scope.project_id || ""}`;
}

// scope draft key -> the commit message typed so far. Module-level on purpose:
// the pane is disposed and remounted on every tab switch and shell rebuild.
const commitDraftStash = new Map();

/** The draft a fresh render should show: a live box is the freshest truth (even
 *  when deliberately cleared); only a remount with no box falls back to the stash. */
export function resolveCommitDraft(liveBoxValue, stash, draftKey) {
  return liveBoxValue !== null ? liveBoxValue : stash.get(draftKey) || "";
}

/** Mirror a draft into the stash: non-empty persists, emptied frees the slot. */
export function syncCommitDraft(stash, draftKey, draft) {
  if (draft) stash.set(draftKey, draft);
  else stash.delete(draftKey);
}

/** Every commit variant leaves the tree committed, so all of them retire the
 *  scope's draft on success — including the bridge-side auto commit. */
export function commitVariantClearsDraft(optionId) {
  return optionId === "commit" || optionId === "agent_commit" || optionId === "auto_commit";
}

// The bridge's terminal git-scope rejections: a pane polling with one of these
// will never recover, so it must show the error instead of "loading..." forever.
const PERMANENT_GIT_SCOPE_ERRORS = [
  "unknown project_id",
  "unknown run_id",
  "unknown worktree_id",
  "provide exactly one of project_id",
];

/** True only for the bridge's permanent scope errors — every other poll failure
 *  stays silent/transient and the poll retries. */
export function isPermanentGitScopeError(message) {
  const text = String(message || "");
  return PERMANENT_GIT_SCOPE_ERRORS.some((known) => text.includes(known));
}

/** The poll's repaint-freeze decision: any in-flight action RPC suppresses the
 *  repaint outright (a repaint would remount the busy split button enabled and
 *  recreate checkboxes mid-RPC); otherwise a rendered pane is left alone while
 *  the key is unchanged, a commit draft is active, or an interaction is live
 *  (an armed inline confirm or an open file menu a repaint would clobber). */
export function pollRenderFrozen({ paneRendered, keyUnchanged, draftActive, actionInFlight, interactionActive = false }) {
  if (actionInFlight) return true;
  return Boolean(paneRendered && (keyUnchanged || draftActive || interactionActive));
}

/** Whether a document-level pointerdown should dismiss the pane's live
 *  interaction: a press OUTSIDE the pane disarms a pending confirm or closes an
 *  open file menu (an inside press never does — the pane's own handlers own
 *  it). Without this, an abandoned menu/confirm freezes the poll indefinitely
 *  (S5), since interactionActive stays true until a click inside disarms it. */
export function outsidePressDismisses({ inside, hasPendingConfirm, fileMenuOpen = false }) {
  if (inside) return false;
  return Boolean(hasPendingConfirm || fileMenuOpen);
}

/** The commit split-button option list: the plain Commit action first (primary),
 *  then whatever agent options the mounting view offers (task scope only).
 *  Commit is commit-all — there is no staged set for it to mean anything else. */
// Run states with a live/parked agent to reach — the states where the
// conversation on this surface can still dispatch work ("Ask agent to commit"
// below). The header button this list was once shared with is gone: talking to
// an agent belongs to the conversation, on the surfaces that host one.
const AGENT_MESSAGEABLE_STATES = ["building", "blocked", "failed", "idle_unreported", "interrupted"];

/** The task-scope agent-commit options for a task's state + goal: "Ask agent to
 *  commit" (task.message) only while the task is messageable, then the Build
 *  auto-commit (task.git_action commit) always. */
export function taskAgentCommitOptions(state, goal) {
  const options = [];
  if (AGENT_MESSAGEABLE_STATES.includes(state))
    options.push({
      id: "agent_commit",
      menuLabel: "Ask agent to commit",
      description: "The agent writes the message",
      busyLabel: "asking agent…",
    });
  options.push({
    id: "auto_commit",
    menuLabel: "Commit all (Build message)",
    description: `Stage everything and commit as Build: ${goal || ""}`,
    busyLabel: "committing…",
  });
  return options;
}

/** Mount the git pane into `container`. Returns { dispose }. `scope` is exactly
 *  one of { project_id } / { run_id }; `callRpc(method, params)` is the RPC
 *  channel; `agentCommitOptions` (run scope) appends to the commit button;
 *  `review` (task/worktree surfaces) plugs the surface's aggregate review diff
 *  in as the rail's "All changes" entry — the rail's top row, and where a
 *  surface that has one opens: { getBase(), mount(host), unmount() }, and the
 *  plug owns the detail pane's DOM while selected (this pane never repaints
 *  over it).
 *  `revisionId()` names the diff revision this surface's comments anchor to. */
// eslint-disable-next-line complexity -- ratchet: mountGitPane is at 21, cap 10 — reduce it, then drop this line
export function mountGitPane(
  container,
  {
    scope,
    callRpc,
    // The cache of the machine this checkout is on: the view hands it down.
    cacheScope,
    agentCommitOptions = [],
    review = null,
    revisionId = () => null,
    // Review prioritization: the run's freshest triage pass (read on every
    // paint — a re-triage lands under this pane), and the project the reviewer's
    // trust dial is remembered for. A surface with neither renders the plain
    // stack it always did.
    triage = () => null,
    triageEnabled = () => false,
    projectId = null,
    // Whose conversation the comments written here belong in: the agent whose
    // bubble is open in the rail beside this pane. Mounted without one (the
    // standalone Files/Changes hosts), the daemon answers with the entity's
    // first agent, which is what this surface always meant.
    agentSelection = createAgentSelection(),
    navigate = null,
    viewingContext = null,
  } = {},
) {
  if (!review && scope?.workspace_id && scope?.source_id) {
    review = createWorkspaceReview({ scope, callRpc, navigate, viewingContext, onBaseChange: () => render() });
  }
  const parsedDiffs = createParsedDiffCache();
  const viewport = createDiffViewport({ repaint: () => renderAndFetch() });
  const openFile = (navigate && navigate.openFile) || null;
  let disposed = false;
  let renderedKey = null; // gitPollKey of the last painted payloads
  let bodiesUnpainted = false; // a file body landed while a repaint was held
  let lastStatus = null;
  let lastLog = null; // the poll's first page (limit default)
  let lastHead; // undefined until the first poll lands
  let extraCommits = []; // "Show more" pages beyond the poll's first page
  let pagedMore = null; // the last fetched page's `more` (null → use lastLog.more)
  // Rail selection, survives repaints. undefined until the first status lands:
  // where the surface opens depends on whether the tree is dirty (a clean branch
  // opens at the commit list with nothing selected and no commit box).
  let selected;
  let reviewMounted = false; // the review plug currently owns the detail host
  let hint = ""; // sticky action hint/error, re-applied after each repaint
  const showCache = new Map(); // hash → git.show payload (commits are immutable)
  // The local cache's address for this checkout. A primary checkout names no
  // entity, so it takes no part — nothing to key by, nothing evicted with it.
  const cacheEntityId = directoryCacheId(scope);
  const cacheAddress = (kind, sub) =>
    cacheEntityId ? cacheScope?.address({ entityId: cacheEntityId, kind, sub }) || null : null;
  const readThroughCache = async (kind, sub) => {
    const address = cacheAddress(kind, sub);
    const record = address ? await readCached(address) : undefined;
    return record ? record.value : undefined;
  };
  const writeThroughCache = (kind, value, sub) => {
    const address = cacheAddress(kind, sub);
    if (address) writeCached(address, value); // fire and forget — never awaited
  };
  // The uncommitted changeset's bodies: git.status names the files and what each
  // one holds, and each file's diff is fetched, cached and answered on its own.
  const fileDiffs = createFileDiffs({
    deviceId: cacheScope?.deviceId,
    entityId: cacheEntityId,
    scope,
    call: callRpc,
    requestScope: cacheScope || callRpc,
  });
  const draftKey = gitDraftKey(scope); // the stash slot for this scope's draft
  let composer = null; // the one box under the diff, mounted once
  // What the reviewer has approved and selected on this surface's files. One
  // set of marks for every changeset it draws, its own and the review plug's,
  // so a file ticked on one is ticked on the other.
  const marks = createReviewMarks();
  let inFlightActions = 0; // commit/discard/sync RPCs currently awaited
  let scopeErrorShown = null; // the terminal scope error currently rendered
  let fileMenuPath = null; // the file whose header ⋯ is open
  const noiseExpanded = new Set(); // changesets whose collapsed noise group is open
  // The triage overlay's reviewer-owned state: which collapsed groups they have
  // opened (keyed by changeset, so opening one on the uncommitted stack says
  // nothing about a commit's), and whether they have dialled the ordering off
  // for this project.
  const expandedGroups = new Map(); // changeset key → the group names opened in it
  const fileFolds = new Map();
  const triageProject = projectId || (scope && scope.project_id) || null;
  let trustDial = loadTrustDial(triageProject);
  // Re-review memory, per changeset: what the reviewer saw when they last sent
  // comments on it, so the next pass can mark what moved. renderedViews is the
  // OPEN changeset's files as the stack draws them, which is what a stamp is of.
  let reviewStamps = new Map();
  let renderedViews = [];
  let uncommittedSource = null;
  let uncommittedSourceViews = [];
  let wholePatchIdentity = null;
  let wholePatchValue = null;
  let pendingConfirm = null; // the armed inline-confirm key (discard/force/abort)
  let armedAt = null; // Date.now() when pendingConfirm was armed (for TTL expiry)
  let drawer = null; // the rail's narrow-viewport pull-out, re-wired per skeleton
  let paintChangesetInto = null;
  let contextFrame = 0;
  let contextCommit = null;

  container.innerHTML = '<div class="gitpane"><div class="empty">loading…</div></div>';

  const contextScroller = () => container.querySelector(".cdetail-host");
  const selectionInsideContext = () => {
    const selection = document.getSelection?.();
    const root = contextScroller();
    return Boolean(
      root && selection && !selection.isCollapsed &&
      (root.contains(selection.anchorNode) || root.contains(selection.focusNode)),
    );
  };
  let contextForce = false;
  const composerFocused = () => {
    const active = document.activeElement;
    return Boolean(active && active.closest(".composer"));
  };
  const paintSelectedContext = () => {
    if (selected === "review") {
      contextCommit = null;
      return;
    }
    if (selected === "uncommitted") {
      contextCommit = null;
      const scroller = contextScroller();
      if (scroller) viewingContext.setVisibleDiffs(scroller, "uncommitted");
      return;
    }
    if (typeof selected === "string" && selected) {
      if (contextCommit !== selected) viewingContext.set({ kind: "commit", sha: selected });
      contextCommit = selected;
      return;
    }
    contextCommit = null;
    viewingContext.clear();
  };
  const syncViewingContext = () => {
    contextFrame = 0;
    if (!viewingContext || disposed) return;
    const forced = contextForce;
    contextForce = false;
    if (!forced && composerFocused()) return;
    paintSelectedContext();
  };
  const scheduleViewingContext = (force = false) => {
    contextForce = contextForce || force;
    if (!viewingContext || contextFrame) return;
    const view = container.ownerDocument.defaultView || globalThis;
    const schedule = view.requestAnimationFrame || ((callback) => view.setTimeout(callback, 0));
    contextFrame = schedule(syncViewingContext);
  };
  const captureViewingSelection = () => {
    if (!viewingContext || selected === "review") return;
    if (selectionInsideContext()) viewingContext.captureDomSelection(contextScroller());
    else {
      const selection = document.getSelection?.();
      if (selection?.isCollapsed && contextScroller()?.contains(selection.anchorNode)) viewingContext.clearSelection();
    }
  };
  const onContextScroll = (event) => {
    if (selected === "uncommitted" && contextScroller()?.contains(event.target)) scheduleViewingContext(true);
  };
  container.addEventListener("scroll", onContextScroll, true);
  document.addEventListener("selectionchange", captureViewingSelection);

  const messageBox = () => container.querySelector(".csinput");
  const draftBusy = () => {
    const box = messageBox();
    return Boolean(box && (box.value.trim() || document.activeElement === box));
  };
  const setHint = (text) => {
    hint = text || "";
    container.querySelectorAll(".githint").forEach((el) => (el.textContent = hint));
  };
  const actionError = (e) => setHint("error: " + ((e && e.message) || "error").slice(0, 70));

  // ---- the ONE inline-confirm arm/disarm path (force push / discard / abort).
  // Every armed confirm is stamped so the poll can auto-expire it, and any
  // other action disarms it — no verb keeps its own bookkeeping.
  const clearConfirm = () => {
    pendingConfirm = null;
    armedAt = null;
  };
  /** Arm `key` (first touch) or fire it (second touch of the same key). Returns
   *  true only on fire; stamps armedAt when it arms so the poll can expire it. */
  const armConfirm = (key) => {
    const decision = resolveInlineConfirm(pendingConfirm, key);
    pendingConfirm = decision.pending;
    armedAt = decision.fire ? null : Date.now();
    return decision.fire;
  };
  /** Disarm any pending confirm; returns whether one was actually cleared so the
   *  caller can decide to repaint. */
  const disarmConfirm = () => {
    if (pendingConfirm === null) return false;
    clearConfirm();
    return true;
  };

  // The persistent skeleton: a left rail drives a right pane whose top carries
  // the toolbar/banner (so the rail runs the pane's full height) and whose
  // scrolling body is the detail host. The row is the shell's two-column
  // primitive, so its width and gutters match every other tab. Each region
  // updates
  // independently so a poll repaint never clobbers the review plug's DOM.
  const paintSkeleton = () => {
    container.innerHTML = `<div class="gitpane">
      <div class="changes2 pane-split">
        <aside class="crail-host pane-list"></aside>
        <section class="cdetail">
          <div class="gp-toolbar"></div>
          <div class="cdetail-host"></div>
          <div class="gp-commit"></div>
        </section>
      </div></div>`;
    // The rail is the drawer on a narrow viewport. Both kinds of row it holds —
    // a set of changes, a commit — put something in the detail column behind
    // it, so both close it; the "show more" row, which only lengthens the rail,
    // does not.
    paintChangesetInto = createChangesetPaint(container.querySelector(".cdetail-host"));
    const split = container.querySelector(".changes2");
    split.insertAdjacentHTML("beforeend", paneDrawerHtml("commits"));
    if (drawer) drawer.dispose();
    drawer = initPaneDrawer(split, { list: split.querySelector(".crail-host"), closeOnSelect: ".rrow, .crow" });
    container.onclick = handleClick;
    container.onkeydown = (event) => {
      if ((event.key === "Enter" || event.key === " ") && event.target.closest(".gitmore")) {
        event.preventDefault();
        showMore();
      }
    };
  };

  const defaultSelection = () => defaultChangesSelection({ status: lastStatus, review });

  const foldsOfOpenChangeset = () => {
    const key = String(selected);
    if (!fileFolds.has(key)) fileFolds.set(key, createFileFolds());
    return fileFolds.get(key);
  };

  // Disagreeing with the pass. Only a run has a pass to disagree with (and a
  // run_id to name in the RPC), so a bare worktree or the primary checkout
  // mounts none and its stack draws no offers.
  const overrides = commentsSupported(scope)
    ? createTriageOverrides({
        post: ({ hunk_id, direction, note }) => {
          if (!triageEnabled()) throw new Error("Review prioritization is turned off.");
          return callRpc("triage.override", { run_id: scope.run_id, hunk_id, direction, note });
        },
        onChange: () => render(),
      })
    : null;

  /** The pass as the reviewer's latest word makes it: what this pane renders,
   *  and what it decides to repaint on. */
  const currentTriage = () => {
    if (!triageEnabled()) return undefined;
    return overrides ? overrides.apply(triage()) : triage();
  };

  /** The freeze key for one poll: the repo's own, plus what the triage overlay
   *  is drawing from. A pass landing (or a re-pass reclassifying) moves nothing
   *  in git, so without it the ordering would wait for the next commit. */
  const pollKeyNow = (status, log) =>
    [gitPollKey(status, log), String(triageEnabled()), triageFingerprint(currentTriage())].join("\x03");

  // Comments are a conversation post, so they exist where there is an agent to
  // post to. One layer serves every changeset: switching selection keeps the
  // pending set (they name their own files), and the tray renders under
  // whichever changeset is open.
  const commentable = commentsSupported(scope);
  const commentLayer = commentable
    ? createCommentLayer({
        readNote: () => (messageBox() ? messageBox().value : ""),
        submit: async (messages) => {
          const reviewedChangeset = selected;
          const reviewedViews = renderedViews;
          const destination = agentSelection.scope();
          const context = viewingContext?.snapshot?.();
          await callRpc("run.request_changes", {
            run_id: scope.run_id,
            ...destination,
            messages: context ? messages.map((message) => ({ ...message, viewing_context: context })) : messages,
            ...MUTATION_THREAD_PAGE,
          });
          viewingContext?.clearSelectionIfMatches?.(context);
          // Stamp what was just reviewed, per changeset: the next pass marks
          // which of ITS files moved since the comments went out.
          reviewStamps = stampChangeset(reviewStamps, reviewedChangeset, reviewedViews);
        },
        revisionId,
        onChange: () => {
          render();
          renderComposer();
        },
      })
    : null;

  /** The triage overlay for the stack being drawn, or null on a surface that
   *  has no pass to read: only a run is triaged, so a bare worktree or the
   *  primary checkout renders the plain stack it always did. The pass is read
   *  fresh on every paint — a re-triage lands under this pane while it is open. */
  const triageOverlay = (patch) => {
    if (!commentsSupported(scope) || !triageEnabled()) return null;
    return {
      triage: currentTriage(),
      patch: patch || "",
      dial: trustDial,
      expandedGroups: expandedGroups.get(String(selected)) || null,
      // The offers are on the hunks of the changeset the pass actually read;
      // a hunk it never named renders none (core/triageModel).
      overridable: Boolean(overrides) && triageEnabled(),
    };
  };

  /** The uncommitted changeset's files as the stack draws them: shape from the
   *  status, bodies from the per-file cache. */
  const uncommittedViews = () => {
    const files = lastStatus.files || [];
    if (files === uncommittedSource) return uncommittedSourceViews;
    uncommittedSource = files;
    uncommittedSourceViews = files.map(fileViewFromStatus);
    return uncommittedSourceViews;
  };

  const uncommittedWholePatch = () => {
    const files = lastStatus.files || [];
    const identity = files
      .map((file) => `${file.path}\x01${fileDiffs.bodyOf(file.path)?.content_key || ""}`)
      .join("\x02");
    if (identity === wholePatchIdentity) return wholePatchValue;
    wholePatchIdentity = identity;
    wholePatchValue = wholePatch(lastStatus, fileDiffs.bodyOf);
    return wholePatchValue;
  };

  /** The one renderer for every changeset: a header, the stacked file diffs in
   *  the folds the reader put them in (noise collapsed into its group at the
   *  bottom), and — where the surface can talk to an agent — the pending-comment
   *  tray. */
  const renderChangeset = (detailHost) => {
    const folds = foldsOfOpenChangeset();
    // Every stack carries the same re-review chip: a file that moved since the
    // reviewer last sent comments on THIS changeset says so.
    const stackFor = (views, patch) => ({
      commentable,
      openable: Boolean(openFile),
      approvable: true,
      approved: marks.approved,
      selectable: true,
      selected: marks.selected,
      noiseExpanded: noiseExpanded.has(String(selected)),
      folds,
      changedSince: changedSinceChangeset(reviewStamps, selected, views),
      ...viewport.renderOptions(),
      // Review prioritization, on the changeset the reviewer has open — the
      // rail is never reordered, only the stack under it. A surface with no run
      // behind it has no pass to read, and a stack whose bodies are still
      // arriving has no whole patch to read one from, so both draw plain.
      review: patch === null ? null : triageOverlay(patch),
    });
    if (selected === "uncommitted") {
      renderedViews = hasUncommittedChanges(lastStatus) ? uncommittedViews() : [];
      // The file's own destructive verb lives behind the header ⋯ — the stage
      // checkboxes it replaced are gone with the staged set.
      const fileMenu = supportsRepoManagement(lastStatus) ? { openPath: fileMenuPath, pendingConfirm } : null;
      paintChangeset(detailHost, {
        bar: uncommittedHeaderHtml(lastStatus),
        views: renderedViews,
        stackOptions: {
          ...stackFor(renderedViews, uncommittedWholePatch()),
          fileMenu,
          bodyOf: fileDiffs.bodyOf,
          empty: "No uncommitted changes.",
        },
      });
      return;
    }
    const detail = showCache.get(selected);
    if (!detail) {
      renderedViews = [];
      detailHost.innerHTML = '<div class="empty cdetail-loading">loading…</div>';
      return;
    }
    // A commit's patch comes whole in its payload, so its files carry their own
    // rows and need no body fetched for them.
    renderedViews = parsedDiffs.views(detail.patch, {
      revision: detail.hash || selected,
      defaultEditedAt: Number(detail.time) * 1000,
    });
    paintChangeset(detailHost, {
      bar: commitHeaderHtml(detail),
      views: renderedViews,
      stackOptions: stackFor(renderedViews, detail.patch),
    });
  };

  const paintChangeset = (detailHost, { bar, views, stackOptions = {} }) => {
    paintChangesetInto({
      bar,
      entries: fileStackEntries(views, stackOptions),
      tray: commentLayer ? commentLayer.trayHtml() : "",
    });
    viewport.attach(detailHost.closest(".cdetail-host") || detailHost);
    if (commentLayer) commentLayer.attach(detailHost);
  };

  /** Keep the open files' bodies current against the shape the pane holds, and
   *  repaint when any land. Fetching is the pane's job, never the render's. A
   *  body that lands while a repaint is held stays unpainted news until the
   *  next turn the pane is free to paint. */
  const refreshBodies = () => {
    if (disposed || !lastStatus || selected !== "uncommitted") return;
    const views = uncommittedViews();
    const folds = foldsOfOpenChangeset();
    const openPaths = new Set(
      views
        .filter((view) => {
          const fold = fileFoldOf(view, { folds, approved: marks.approved });
          return viewport.shouldLoad(fileKey(view), fold);
        })
        .map((view) => view.path),
    );
    fileDiffs
      .sync({
        status: lastStatus,
        openPaths,
      })
      .then(
        (filled) => {
          if (!filled || disposed) return;
          bodiesUnpainted = repaintHeld({ keyUnchanged: false });
          if (!bodiesUnpainted) render();
        },
        (error) => {
          // A body fetch failure is transient — the next turn asks again; a
          // scope that no longer resolves is the terminal error git.status
          // reports.
          if (!disposed && isPermanentGitScopeError(error && error.message)) renderScopeError(error.message);
        },
      );
  };

  /** Paint what is held, then ask for whatever the paint found missing. */
  const renderAndFetch = () => {
    render();
    refreshBodies();
  };

  /// Whose comments the box under the diff is writing: the review plug's while
  /// its changeset is on screen, this pane's own otherwise. One box, and it
  /// always speaks to the changeset the reviewer is looking at.
  /** This pane's own comment layer, answering the same two questions the review
   *  plug does, so the box below can ask one thing without knowing which. */
  const commentLayerAsPlug = {
    commentOffer: () => ({ commentable: Boolean(commentLayer), pending: commentLayer ? commentLayer.count() : 0 }),
    sendComments: () => (commentLayer ? commentLayer.send() : Promise.resolve()),
  };

  const activeComments = () => (reviewMounted ? review : commentLayerAsPlug);

  const activeCommentOffer = () =>
    activeComments()?.commentOffer?.() || { commentable: false, pending: 0 };

  /// What the box can do right now.
  ///
  /// Committing is offered on the two changesets that SHOW uncommitted work —
  /// the aggregate and Uncommitted itself. On a past commit it would commit the
  /// worktree, which is not what the reviewer is looking at.
  const composerOffer = () => {
    const offer = activeCommentOffer();
    const commitsHere = selected === "uncommitted" || selected === "review";
    return {
      commentable: offer.commentable,
      pendingComments: offer.pending,
      uncommitted: commitsHere && commitBoxVisible(lastStatus),
      commitExtras: agentCommitOptions,
    };
  };

  /// The verb the reviewer picked, with what they wrote in the box.
  const runComposerVerb = (optionId, text) =>
    optionId === "comment" ? activeComments()?.sendComments?.() || Promise.resolve() : runCommitOption(optionId, text);

  /** The one box under the diff: below the scroller, so reading to the bottom of
   *  a long stack never takes it off screen. Mounted once and kept — it holds a
   *  draft, and rebuilding it would take the caret with it. */
  const renderComposer = () => {
    const commitHost = container.querySelector(".gp-commit");
    if (!commitHost) return;
    if (!composer)
      composer = mountChangesComposer(commitHost, {
        offers: composerOffer,
        run: runComposerVerb,
        readDraft: () => resolveCommitDraft(null, commitDraftStash, draftKey),
        // Every keystroke lands in the stash so tab switches and view-shell
        // rebuilds (which remount the pane from scratch) restore the draft.
        writeDraft: (value) => syncCommitDraft(commitDraftStash, draftKey, value),
      });
    else composer.refresh();
    setHint(hint); // the box the refresh may have rebuilt has no hint in it yet
    // A repaint during an in-flight action must not resurrect an enabled button
    // (double-fire) — leave it disabled until the RPC settles.
    if (inFlightActions > 0) {
      const primaryButton = commitHost.querySelector(".btn:not(.caret)");
      if (primaryButton) primaryButton.disabled = true;
    }
  };

  // eslint-disable-next-line complexity -- ratchet: this callback is at 14, cap 10 — reduce it, then drop this line
  const render = () => {
    if (disposed || !lastStatus || !lastLog) return;
    if (!container.querySelector(".changes2")) paintSkeleton();
    const repoControls = supportsRepoManagement(lastStatus);
    // Always drawn, even against a bridge too old to manage the repo: the merge
    // verb the surface hosts here is the surface's own, not the bridge's. One
    // bar carries everything that must survive a scroll — the branch's standing,
    // the verbs, and whatever the selection raises.
    container.querySelector(".gp-toolbar").innerHTML = gitToolbarHtml({
      chips: syncChipState(lastStatus),
      stat: lastStatus.stat || null,
      selected: marks.selected.size,
      banner: repoControls ? repoStateBanner(lastStatus.repo_state) : null,
      pendingConfirm,
      repo: repoControls,
    });
    const mergedLog = {
      ...lastLog,
      commits: [...(lastLog.commits || []), ...extraCommits],
      more: pagedMore ?? lastLog.more,
    };
    paintRail({
      review: review ? { base: review.getBase(), subtitle: review.getRailSubtitle?.() } : null,
      status: lastStatus,
      log: mergedLog,
      selected,
    });
    const detailHost = container.querySelector(".cdetail-host");
    if (selected === "review") {
      // The plug owns the detail DOM — mount once, then leave it alone.
      if (!reviewMounted) {
        detailHost.innerHTML = "";
        // The plug's own git verb — merging — goes in the git toolbar above the
        // stack. The toolbar is rewritten on every paint, so the plug is asked
        // for a host each time rather than handed the element.
        review.mount(detailHost, {
          gitActions: () => container.querySelector(".gtmerge"),
          readNote: () => (messageBox() ? messageBox().value : ""),
          // The box's own button says how many comments the send carries.
          onComments: () => renderComposer(),
          reviewMarks: marks,
          // The bar above the stack names what is selected, and the bar is this
          // pane's — a mark made inside the plug has to reach it.
          onMarks: () => render(),
        });
        reviewMounted = true;
      }
      // The toolbar this render just rewrote took the plug's button with it.
      if (review.refreshActions) review.refreshActions();
    } else {
      if (reviewMounted) {
        review.unmount();
        reviewMounted = false;
        detailHost.innerHTML = "";
      }
      timedPaint("changes", () =>
        paintKeepingPlace(
          detailHost,
          () => {
            if (selected === null || selected === undefined) {
              detailHost.innerHTML = changesetPlaceholderHtml("Pick a commit to see what changed.");
              return;
            }
            renderChangeset(detailHost);
          },
          DIFF_PLACE_KEEPING,
        ),
      );
    }
    renderComposer();
    setHint(hint);
    mountToolbarControls();
    // Every toolbar verb stays disabled through an in-flight action (a repaint
    // mid-action must not resurrect a live button to double-fire); the action
    // wrapper re-enables them once the RPC settles.
    if (toolbarControlsDisabled(inFlightActions)) disableToolbarControls();
    syncViewingContext();
  };

  /// The rail, reconciled row by row rather than rewritten.
  ///
  /// Every row it holds has a name — "uncommitted", a commit's sha, the labels
  /// and affordances between them — so a poll that added one commit inserts one
  /// row, and the rest of the rail and its scroll position are exactly where
  /// they were. Nothing in the rail is wired to a row: the surface's one click
  /// handler reads which row was pressed off the DOM.
  const paintRail = (parts) => {
    const host = container.querySelector(".crail-host");
    const rail = host.querySelector(".crail") || host.appendChild(el('<div class="crail"></div>'));
    patchList(rail, changesRailEntries(parts), { keyOf: (entry) => entry.key, render: (entry) => entry.html });
  };

  /** Mount the Pull/Push/Stash split buttons into their toolbar hosts. Each host
   *  is absent unless the repo-management toolbar rendered (older bridge → no
   *  hosts, nothing to mount). The git bar is a dense toolbar, so its verbs wear
   *  the mini button — the same one Fetch wears. */
  const mountToolbarControls = () => {
    const pullHost = container.querySelector(".gtpull");
    if (pullHost) mountSplitButton(pullHost, { options: pullSplitOptions(), run: runSyncOption, variant: TOOLBAR_BUTTON_VARIANT });
    const pushHost = container.querySelector(".gtpush");
    // Arming force push re-renders with the armed label so the menu item reads
    // "Confirm force push?" — the pending two-click confirm is visible (S2a).
    if (pushHost)
      mountSplitButton(pushHost, {
        options: pushSplitOptions(pendingConfirm === "force_push"),
        run: runSyncOption,
        variant: TOOLBAR_BUTTON_VARIANT,
      });
    const stashHost = container.querySelector(".gtstash");
    if (stashHost)
      mountSplitButton(stashHost, {
        options: stashSplitOptions(lastStatus && lastStatus.stash_count),
        run: runSyncOption,
        variant: TOOLBAR_BUTTON_VARIANT,
      });
  };

  const toolbarButtons = () => [...container.querySelectorAll(".gtsync .btn, .gtstash .btn")];
  const disableToolbarControls = () => toolbarButtons().forEach((b) => (b.disabled = true));

  /** The ONE settle re-enable, shared by every action wrapper: revive every
   *  toolbar verb AND the commit primary from a single selector list, so no
   *  action can settle leaving another action's button stuck disabled (S1). */
  const reenableAllControls = () =>
    settleReenableSelectors().forEach((selector) =>
      container.querySelectorAll(selector).forEach((button) => (button.disabled = false)),
    );

  /** The ONE settle path: decrement the in-flight counter and, once the last
   *  action settles, re-enable everything. Every action wrapper funnels through
   *  this so their bookkeeping can never diverge. */
  const settleInFlight = () => {
    inFlightActions -= 1;
    if (!disposed && actionSettleReenables(inFlightActions)) reenableAllControls();
  };

  const paintFrom = (status, log) => {
    lastStatus = status;
    if (log) lastLog = log;
    // A content refresh clears any stale armed confirm (the file/state it named
    // may be gone) — matching "any repaint resets the pending confirm".
    clearConfirm();
    renderedKey = pollKeyNow(lastStatus, lastLog);
    renderAndFetch();
  };

  /** Refetch both payloads and repaint unconditionally (post-action refresh). */
  const forceRefresh = async () => {
    let status, log;
    try {
      [status, log] = await Promise.all([callRpc("git.status", { ...scope }), callRpc("git.log", { ...scope })]);
    } catch (e) {
      actionError(e);
      return;
    }
    if (disposed) return;
    lastHead = status.head;
    extraCommits = [];
    pagedMore = null;
    paintFrom(status, log);
  };

  /** Drop the scope's draft everywhere it lives: the stash and the live box. */
  const clearCommitDraft = () => {
    commitDraftStash.delete(draftKey);
    const box = messageBox();
    if (box) box.value = "";
  };

  /** Run a commit variant with the in-flight guard: while any action RPC is
   *  awaited, poll repaints are suppressed and remounted buttons stay disabled.
   *  Once settled, a button remounted disabled mid-action is re-enabled. */
  const runCommitOption = async (optionId, message) => {
    inFlightActions += 1;
    try {
      return await performCommitOption(optionId, String(message || "").trim());
    } finally {
      settleInFlight();
    }
  };

  // eslint-disable-next-line complexity -- ratchet: this callback is at 18, cap 10 — reduce it, then drop this line
  const performCommitOption = async (optionId, message) => {
    setHint("");
    if (optionId === "commit") {
      if (!message) {
        setHint("Enter a commit message first.");
        throw new Error("commit message must not be empty");
      }
      // The files the reviewer ticked, or the whole worktree when they ticked
      // none — staged first, then committed together.
      const paths = commitPaths(commitAllPaths(lastStatus), marks.selected);
      if (!paths.length) {
        setHint("Nothing to commit.");
        throw new Error("nothing to commit");
      }
      let result;
      try {
        await callRpc("git.stage", { ...scope, paths });
        result = await callRpc("git.commit", { ...scope, message });
      } catch (e) {
        actionError(e);
        throw e;
      }
      if (disposed) return;
      if (commitVariantClearsDraft(optionId)) clearCommitDraft();
      lastHead = result.status.head;
      extraCommits = [];
      pagedMore = null;
      paintFrom(result.status); // repaint from the returned status immediately…
      try {
        const log = await callRpc("git.log", { ...scope }); // …then pull the new commit into history
        if (!disposed) paintFrom(lastStatus, log);
      } catch {
        /* the poll catches the log up */
      }
      return;
    }
    if (optionId === "agent_commit") {
      const context = viewingContext?.snapshot?.();
      try {
        await callRpc("run.message", {
          run_id: scope.run_id,
          message: AGENT_COMMIT_MESSAGE,
          ...(context ? { viewing_context: context } : {}),
          ...MUTATION_THREAD_PAGE,
        });
        viewingContext?.clearSelectionIfMatches?.(context);
      } catch (e) {
        actionError(e);
        throw e;
      }
      if (disposed) return;
      if (commitVariantClearsDraft(optionId)) clearCommitDraft();
      setHint("Asked the agent to commit.");
      render(); // remount the split button so it re-enables
      return;
    }
    if (optionId === "auto_commit") {
      try {
        await callRpc("run.git_action", { run_id: scope.run_id, action: "commit", ...MUTATION_THREAD_PAGE });
      } catch (e) {
        actionError(e);
        throw e;
      }
      if (disposed) return;
      if (commitVariantClearsDraft(optionId)) clearCommitDraft();
      await forceRefresh();
      return;
    }
    throw new Error(`unknown commit option ${optionId}`);
  };

  /** Switch the rail selection. A commit selection paints its cached detail
   *  immediately (or a loading placeholder while git.show is in flight). */
  const selectRail = (sel) => {
    if (selected === sel) return;
    selected = sel;
    viewingContext?.clearSelection();
    clearConfirm();
    fileMenuPath = null; // a menu belongs to the changeset it was opened on
    renderAndFetch();
    if (sel !== "review" && sel !== "uncommitted" && !showCache.has(sel)) fetchShow(sel);
  };

  const fetchShow = async (hash) => {
    // A commit is immutable, so the local cache answers for the bridge
    // outright — no revalidation, no second ask, ever.
    const cached = await readThroughCache("show", hash);
    if (disposed) return;
    if (cached) {
      showCache.set(cached.hash, cached);
      if (selected === hash) render();
      return;
    }
    let show;
    try {
      show = await callRpc("git.show", { ...scope, hash });
    } catch (e) {
      if (disposed) return;
      actionError(e);
      if (selected === hash) {
        selected = defaultSelection();
        render();
      }
      return;
    }
    showCache.set(show.hash, show);
    writeThroughCache("show", show, show.hash);
    if (!disposed && selected === hash) render();
  };

  const showMore = async () => {
    const skip = ((lastLog && lastLog.commits) || []).length + extraCommits.length;
    let page;
    try {
      page = await callRpc("git.log", { ...scope, skip });
    } catch (e) {
      actionError(e);
      return;
    }
    if (disposed) return;
    extraCommits = [...extraCommits, ...(page.commits || [])];
    pagedMore = Boolean(page.more);
    render();
  };

  // ---- repo-management actions (v2 toolbar / banner / discard) ----

  /** Run a repo-management mutation under the shared in-flight guard: poll
   *  repaints freeze and every toolbar verb stays disabled while awaited, then
   *  the controls re-enable once the RPC settles. Mirrors runCommitOption. */
  const runGuarded = async (work) => {
    // Attempting ANY guarded action disarms a pending confirm at entry — success
    // or failure (S2b). A firing confirm already cleared itself in armConfirm, so
    // this only bites a *stale* arm from a different verb (e.g. an abandoned
    // force-push arm when the user clicks Fetch), preventing a later single click
    // from firing it without a fresh confirm.
    const disarmed = disarmConfirm();
    inFlightActions += 1;
    disableToolbarControls();
    if (disarmed) render(); // repaint so the armed force-push label reverts
    try {
      return await work();
    } finally {
      settleInFlight();
    }
  };

  /** Apply a status-returning mutation's result: HEAD/paging reset + repaint,
   *  then pull the fresh log in (a sync/checkout can rewrite history). */
  const applyStatusResult = async (status, successHint) => {
    if (disposed) return;
    lastHead = status.head;
    extraCommits = [];
    pagedMore = null;
    if (successHint !== undefined) setHint(successHint);
    else setHint("");
    paintFrom(status);
    try {
      const log = await callRpc("git.log", { ...scope });
      if (!disposed) paintFrom(lastStatus, log);
    } catch {
      /* the poll catches the log up */
    }
  };

  /** Fetch / Pull / Push / Stash / Pop. force_push is gated behind one inline
   *  confirm; the rest fire immediately. Errors set the hint and re-throw so a
   *  split button restores itself. */
  const runSyncOption = async (optionId) => {
    setHint("");
    if (syncActionNeedsConfirm(optionId)) {
      if (!armConfirm(optionId)) {
        setHint("Force push (with lease) — click Force push again to confirm.");
        render(); // re-render with the armed label so the confirm is visible (S2a)
        throw new Error("confirm required");
      }
    }
    const { method, params } = syncActionRpc(optionId);
    return runGuarded(async () => {
      let status;
      try {
        status = await callRpc(method, { ...scope, ...params });
      } catch (e) {
        if (!disposed) actionError(e);
        throw e; // let the split button restore itself
      }
      await applyStatusResult(status);
    });
  };

  const runFetch = () =>
    runGuarded(async () => {
      let status;
      try {
        status = await callRpc("git.fetch", { ...scope });
      } catch (e) {
        if (!disposed) actionError(e);
        return;
      }
      await applyStatusResult(status);
    });

  const runDiscard = (path) =>
    runGuarded(async () => {
      let status;
      try {
        status = await callRpc("git.discard", { ...scope, paths: [path] });
      } catch (e) {
        if (!disposed) actionError(e);
        return;
      }
      // The menu belonged to a file that may not exist any more — and an open
      // menu freezes the poll, so it closes with the action that fired from it.
      fileMenuPath = null;
      await applyStatusResult(status);
    });

  const runAbort = () =>
    runGuarded(async () => {
      let status;
      try {
        status = await callRpc("git.merge_abort", { ...scope });
      } catch (e) {
        if (!disposed) actionError(e);
        return;
      }
      await applyStatusResult(status);
    });

  /** The inline-confirm gate for a destructive click: a first click arms and
   *  repaints the armed label; a second on the same control fires `action`. */
  const confirmThen = (key, action) => {
    if (inFlightActions > 0) return;
    if (armConfirm(key)) action();
    else render();
  };

  const claimSecret = (event) => toggleSecretSpoiler(event.target);

  const claimFetch = (event) => {
    if (!event.target.closest(".gtfetch")) return false;
    runFetch();
    return true;
  };

  const claimSyncButtonWiredElsewhere = (event) => Boolean(event.target.closest(".gtsync") || event.target.closest(".gtstash"));

  const claimAbort = (event) => {
    if (!event.target.closest(".gitabort")) return false;
    confirmThen("abort", runAbort);
    return true;
  };

  const claimDiscard = (event) => {
    const button = event.target.closest(".gitdiscard");
    if (!button) return false;
    const path = pathOf(button.dataset.key);
    confirmThen(`discard:${path}`, () => runDiscard(path));
    return true;
  };

  const claimFileMenu = (event) => {
    const button = event.target.closest(".fmenu");
    if (!button) return false;
    const path = pathOf(button.dataset.key);
    fileMenuPath = fileMenuPath === path ? null : path;
    clearConfirm();
    render();
    return true;
  };

  const claimOverride = (event) => Boolean(!reviewMounted && overrides && overrides.handleClick(event));

  const claimTrustDial = (event) => {
    if (!event.target.closest(".tdial")) return false;
    trustDial = !trustDial;
    saveTrustDial(triageProject, trustDial);
    render();
    return true;
  };

  const claimTriageGroup = (event) => {
    const head = event.target.closest(".tgrouphead");
    if (!head) return false;
    const key = String(selected);
    if (!expandedGroups.has(key)) expandedGroups.set(key, new Set());
    const opened = expandedGroups.get(key);
    const name = head.dataset.group;
    if (opened.has(name)) opened.delete(name);
    else opened.add(name);
    render();
    return true;
  };

  const claimNoiseGroup = (event) => {
    if (!event.target.closest(".noisehead")) return false;
    const key = String(selected);
    if (noiseExpanded.has(key)) noiseExpanded.delete(key);
    else noiseExpanded.add(key);
    render();
    return true;
  };

  const claimRailRow = (event) => {
    const pinned = event.target.closest(".rrow[data-sel]");
    const commit = event.target.closest(".crow[data-hash]");
    if (!pinned && !commit) return false;
    selectRail(pinned ? pinned.dataset.sel : commit.dataset.hash);
    return true;
  };

  /// A mark made on one of this pane's OWN stacks. The review plug draws its
  /// own and handles its own; the marks themselves are shared, so a file ticked
  /// on either is ticked on both.
  const claimFileMark = (event) => {
    if (reviewMounted) return false;
    const approve = event.target.closest(".fapprove");
    const select = event.target.closest(".fselect-box");
    if (!approve && !select) return false;
    const key = (approve || select).dataset.key;
    if (approve) marks.toggleApproved(pathOf(key));
    else marks.toggleSelected(pathOf(key));
    renderAndFetch();
    renderComposer();
    return true;
  };

  /// The verbs the selection raises, aimed at every file in hand at once.
  const claimSelectionVerb = (event) => {
    const button = event.target.closest(".selapprove, .selclear");
    if (!button) return false;
    if (button.classList.contains("selapprove")) marks.approveSelected();
    else marks.clearSelection();
    // A mark the plug shares has to reach the stack the plug is drawing.
    if (reviewMounted && review.refresh) review.refresh();
    renderAndFetch();
    renderComposer();
    return true;
  };

  const claims = [
    claimSecret,
    claimFileMark,
    claimSelectionVerb,
    claimFetch,
    claimSyncButtonWiredElsewhere,
    claimAbort,
    claimDiscard,
    claimFileMenu,
    claimOverride,
    claimTrustDial,
    claimTriageGroup,
    claimNoiseGroup,
    claimRailRow,
    ...stackClaims({
      comments: () => (reviewMounted ? null : commentLayer),
      openFile: () => openFile,
      folds: () => (reviewMounted ? null : foldsOfOpenChangeset()),
      approved: () => marks.approved,
      repaint: renderAndFetch,
    }),
  ];

  const handleClick = (event) => {
    const file = event.target.closest?.(".file[data-key]");
    const foldPress = Boolean(
      file &&
      !event.target.closest("button, input, label") &&
      (event.target.closest(".fhead") || file.classList.contains("capped")),
    );
    for (const claim of claims)
      if (claim(event)) {
        if (foldPress) viewport.request(file.dataset.key);
        return;
      }
    if (disarmConfirm()) render();
    if (event.target.closest(".gitmore")) showMore();
  };

  /** A permanent scope rejection replaces the pane body (there is nothing to
   *  retry: the task/project this scope named no longer resolves). */
  const renderScopeError = (message) => {
    if (scopeErrorShown === message) return;
    scopeErrorShown = message;
    if (reviewMounted) {
      review.unmount(); // its host is about to be wiped with the skeleton
      reviewMounted = false;
    }
    container.innerHTML = `<div class="gitpane"><div class="empty giterror">${esc(message)}</div></div>`;
  };

  /** An interaction a repaint would clobber: an armed confirm, an open file or
   *  split menu somebody is reaching into, or a review in progress (pending
   *  comments, an open popover, typed general text). Each closes itself on a
   *  press outside, so none can hold a paint longer than the reach. */
  const interactionLive = () =>
    Boolean(pendingConfirm) ||
    fileMenuPath !== null ||
    Boolean(container.querySelector(".splitmenu:not([hidden])")) ||
    Boolean(commentLayer && commentLayer.busy());

  /** Whether the pane must keep its hands off the DOM right now — asked by every
   *  paint the pane does on its own initiative, not just the poll's. */
  const repaintHeld = ({ keyUnchanged }) =>
    pollRenderFrozen({
      paneRendered: Boolean(container.querySelector(".gitpane .changes2")),
      keyUnchanged,
      draftActive: draftBusy(),
      actionInFlight: inFlightActions > 0,
      interactionActive: interactionLive(),
    });

  // eslint-disable-next-line complexity -- ratchet: this callback is at 16, cap 10 — reduce it, then drop this line
  const poll = async () => {
    if (disposed) return;
    let status, log;
    try {
      [status, log] = await Promise.all([
        callRpc("git.status", { ...scope, ...ifStatusKey(lastStatus) }),
        callRpc("git.log", { ...scope }),
      ]);
    } catch (e) {
      // Permanent scope errors (pruned task, removed project) never recover —
      // surface them instead of "loading…" forever; everything else is
      // transient and the poll retries silently.
      if (!disposed && isPermanentGitScopeError(e && e.message)) renderScopeError((e && e.message) || "error");
      return;
    }
    if (disposed) return;
    scopeErrorShown = null; // recovered — the next render paints normally
    status = statusAfterPoll(status, lastStatus);
    if (lastHead !== undefined && status.head !== lastHead) {
      extraCommits = []; // HEAD moved — the paged-in history is stale
      pagedMore = null;
    }
    lastHead = status.head;
    lastStatus = status;
    lastLog = log;
    // Every live poll writes the shape through as received: it carries no patch
    // to strip, and each file's body is its own record.
    writeThroughCache("status", status);
    writeThroughCache("log", log);
    // Where the surface opens is a function of what it has to show: the first
    // status picks it, and an empty selection falls back to the same place
    // afterwards.
    selected = selected === undefined ? defaultSelection() : selectionAfterPoll(selected, status, { review });
    // Every turn asks for whatever the open files are missing, freeze or not: a
    // body fetch that failed leaves the shape where it was, so a retry gated on
    // the shape moving would never come. A quiet repo asks for nothing.
    refreshBodies();
    // An abandoned confirm auto-expires: past the TTL the poll disarms it and
    // forces a repaint (S2c), so a destructive verb never stays one click from
    // firing — and the interactionActive freeze it caused is released too.
    const expired = confirmExpired(armedAt, Date.now());
    if (expired) clearConfirm();
    const key = pollKeyNow(status, log);
    // Freeze while nothing has moved, while the user is drafting a commit
    // message, while any action RPC is in flight, or while an interaction is
    // live. A body that landed unpainted is something that moved; a just-expired
    // confirm bypasses the freeze so its armed label actually clears.
    if (!expired && repaintHeld({ keyUnchanged: key === renderedKey && !bodiesUnpainted })) return;
    renderedKey = key;
    bodiesUnpainted = false;
    render();
  };

  // A press anywhere outside the pane dismisses a live interaction (an armed
  // confirm or an open file menu), mirroring splitButton's own outside-close.
  // Without it an abandoned confirm/menu keeps interactionActive true and
  // freezes the poll until the user clicks back inside (S5). Removed on dispose.
  const onOutsidePointerDown = (event) => {
    if (
      !outsidePressDismisses({
        inside: container.contains(event.target),
        hasPendingConfirm: pendingConfirm !== null,
        fileMenuOpen: fileMenuPath !== null,
      })
    )
      return;
    clearConfirm();
    fileMenuPath = null; // an abandoned file menu must not freeze the poll
    render();
  };
  document.addEventListener("pointerdown", onOutsidePointerDown);

  /** The synced status and commit list, painted while the first poll is still
   *  in flight. The live answer wins any race — a seed that arrives second
   *  drops itself. */
  const seedFromCache = async () => {
    const [cachedStatus, cachedLog] = await Promise.all([readThroughCache("status"), readThroughCache("log")]);
    if (disposed || lastStatus || !cachedStatus || !cachedLog) return;
    lastHead = cachedStatus.head;
    lastStatus = cachedStatus;
    lastLog = cachedLog;
    if (selected === undefined) selected = defaultSelection();
    renderAndFetch();
  };

  seedFromCache();
  poll();
  // The pane reads one checkout, so it refetches when that checkout's entity
  // moves — the git watcher stales a run the instant files land in it. A
  // project's own checkout is not an entity the bridge names, and its state
  // moves with the feed, so that scope watches the board instead.
  const watcher = watchChanges({
    refresh: poll,
    intervalMs: GIT_PANE_POLL_MS,
    entity: scope.workspace_id || scope.run_id || scope.worktree_id || null,
    // A workspace source is watched at its own cadence rather than standing
    // down to the safety poll: the bridge does not push for every source in a
    // multi-source workspace, so the interval stays where it has always been.
    keepPolling: Boolean(scope.workspace_id),
    // Focus tier. A project's own checkout is no entity the bridge names, so
    // that scope watches the board — where `state` is all there is, and the
    // manager trims the ask to it.
    kinds: ["state", "git", "files"],
    mode: "realtime",
  });
  const editedTimeWatcher = watchEditedTimes(container);

  return {
    dispose() {
      disposed = true;
      watcher.dispose();
      editedTimeWatcher.dispose();
      document.removeEventListener("pointerdown", onOutsidePointerDown);
      if (reviewMounted) {
        review.unmount(); // stop the plug's poll; the view may remount it later
        reviewMounted = false;
      }
      if (drawer) {
        drawer.dispose();
        drawer = null;
      }
      fileDiffs.dispose();
      parsedDiffs.clear();
      viewport.dispose();
      container.removeEventListener("scroll", onContextScroll, true);
      document.removeEventListener("selectionchange", captureViewingSelection);
      if (contextFrame) {
        const view = container.ownerDocument.defaultView || globalThis;
        (view.cancelAnimationFrame || view.clearTimeout).call(view, contextFrame);
        contextFrame = 0;
      }
      if (viewingContext && selected !== "review") viewingContext.clear();
      if (commentLayer) commentLayer.dispose();
      if (overrides) overrides.dispose();
      container.onclick = null;
    },
  };
}
