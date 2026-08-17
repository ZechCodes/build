// DOM-light controller for the Changes surface: a left rail (Uncommitted at the
// top with its +/− counts, the commit list under it, and — where a view plugs
// one in — the review aggregate below that) driving a right detail pane. Every
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
import {
  gitBranchControlHtml,
  gitToolbarHtml,
  gitStateBannerHtml,
  branchMenuHtml,
  moveActiveIndex,
  AGENT_COMMIT_MESSAGE,
} from "./gitRender.js";
import {
  changesRailEntries,
  uncommittedHeaderHtml,
  commitHeaderHtml,
  commitBoxHtml,
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
import { parseDiff } from "./diff.js";
import { diffStackHtml } from "./diffRender.js";
import { initPaneDrawer, paneDrawerHtml } from "./paneDrawer.js";
import { mountSplitButton } from "./splitButton.js";
import { toggleSecretSpoiler } from "./secrets.js";
import { watchChanges } from "./changeEvents.js";
import { patchList } from "./patchList.js";
import { el } from "../dom.js";

export const GIT_PANE_POLL_MS = 1600;

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

/** Branch switching belongs to the checkouts the human owns — the project's
 *  primary one and its worktrees. A run worktree's branch is owned by the run
 *  lifecycle, so sessions show it as static text instead. */
export function showBranchControl(scope) {
  return Boolean(scope && scope.project_id && !scope.run_id);
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
 *  of secondary actions, so Fetch, the branch control and the Pull/Push/Stash
 *  split buttons are all the app's mini button — no look of its own. */
export const TOOLBAR_BUTTON_VARIANT = "mini";

/** The controls one settled action re-enables — every toolbar verb PLUS the
 *  commit primary. This is the single source S1 unifies on: a toolbar action's
 *  repaint disables the commit button (render() disables it while any action is
 *  in flight), so the SAME settle that re-enables the toolbar must also re-enable
 *  Commit, or a Fetch/Push leaves it stuck disabled. */
export function settleReenableSelectors() {
  return [
    ".gtfetch",
    ".gtbranchbtn",
    ".gtsync .btn",
    ".gtstash .btn",
    ".gitcommit-actions .btn.primary:not(.caret)",
  ];
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
 *  (discard, force push, branch delete, merge abort): a first touch arms the
 *  control (returns its key as the new pending); a second touch of the SAME
 *  control fires and disarms; touching a different control re-arms that one. */
export function resolveInlineConfirm(pending, key) {
  if (pending === key) return { fire: true, pending: null };
  return { fire: false, pending: key };
}

/** How long an armed inline confirm stays live before the poll auto-disarms it.
 *  A destructive verb (force push / discard / abort / branch delete) armed and
 *  then abandoned must not stay one click from firing indefinitely. */
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

/** The repaint-freeze key for one poll's payloads: HEAD + branch + the status
 *  patch + every file's stage state + the visible commit page, PLUS the additive
 *  v2 sync fields (repo_state/upstream/ahead/behind/stash_count) so an
 *  out-of-band ref move — a terminal `git fetch` shifting `behind` with no local
 *  HEAD/file change — still repaints the toolbar chips and state banner.
 *  Unchanged key → the poll leaves the DOM (and the user's checkbox focus) alone. */
export function gitPollKey(status, log, nowSeconds = Date.now() / 1000) {
  const files = (status.files || [])
    .map((f) => [f.path, f.staged, f.index_status, f.worktree_status].join("\x01"))
    .join("\x02");
  const commits = ((log && log.commits) || []).map((c) => c.hash).join(",");
  // A coarse minute bucket: relative commit ages re-render at most once a
  // minute even when the repo itself is untouched.
  const minuteBucket = Math.floor(nowSeconds / 60);
  return [
    status.branch,
    status.head,
    status.truncated,
    Boolean(status.files_truncated),
    status.patch,
    files,
    commits,
    Boolean(log && log.more),
    status.repo_state,
    status.upstream,
    status.ahead,
    status.behind,
    status.stash_count,
    minuteBucket,
  ].join("\x03");
}

/** The stash key for a scope's in-progress commit-message draft: drafts live in
 *  a module-level Map so tab switches and view-shell rebuilds (which remount the
 *  pane from scratch) restore them transparently. Keyed on the narrowest id the
 *  scope carries — a worktree scope names its project too, and keying on that
 *  would pool every worktree's draft with the project's own. */
export function gitDraftKey(scope) {
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
 *  (an armed inline confirm or an open branch menu a repaint would clobber). */
export function pollRenderFrozen({ paneRendered, keyUnchanged, draftActive, actionInFlight, interactionActive = false }) {
  if (actionInFlight) return true;
  return Boolean(paneRendered && (keyUnchanged || draftActive || interactionActive));
}

/** Whether a document-level pointerdown should dismiss the pane's live
 *  interaction: a press OUTSIDE the pane closes an open branch menu or disarms a
 *  pending confirm (an inside press never does — the pane's own handlers own
 *  it). Without this, an abandoned menu/confirm freezes the poll indefinitely
 *  (S5), since interactionActive stays true until a click inside disarms it. */
export function outsidePressDismisses({ inside, branchMenuOpen, hasPendingConfirm, fileMenuOpen = false }) {
  if (inside) return false;
  return Boolean(branchMenuOpen || hasPendingConfirm || fileMenuOpen);
}

/** The commit split-button option list: the plain Commit action first (primary),
 *  then whatever agent options the mounting view offers (task scope only).
 *  Commit is commit-all — there is no staged set for it to mean anything else. */
export function commitSplitOptions(agentCommitOptions = []) {
  return [
    { id: "commit", label: "Commit", description: "commit everything in the worktree with your message", busyLabel: "Committing…" },
    ...agentCommitOptions,
  ];
}

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
 *  in as the rail's "All changes" entry — under the commit list, never the
 *  default selection: { getBase(), mount(host), unmount() }, and the plug owns
 *  the detail pane's DOM while selected (this pane never repaints over it).
 *  `revisionId()` names the diff revision this surface's comments anchor to;
 *  `onNavigate` is reserved for future cross-surface links. */
export function mountGitPane(
  container,
  {
    scope,
    callRpc,
    agentCommitOptions = [],
    review = null,
    revisionId = () => null,
    onNavigate = null,
    // Review prioritization: the run's freshest triage pass (read on every
    // paint — a re-triage lands under this pane), and the project the reviewer's
    // trust dial is remembered for. A surface with neither renders the plain
    // stack it always did.
    triage = () => null,
    projectId = null,
    // Whose conversation the comments written here belong in: the agent whose
    // bubble is open in the rail beside this pane. Mounted without one (the
    // standalone Files/Changes hosts), the daemon answers with the entity's
    // first agent, which is what this surface always meant.
    agentSelection = createAgentSelection(),
  } = {},
) {
  void onNavigate; // accepted per the pane contract; no link targets yet
  let disposed = false;
  let renderedKey = null; // gitPollKey of the last painted payloads
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
  const draftKey = gitDraftKey(scope); // the stash slot for this scope's draft
  let inFlightActions = 0; // commit/discard/sync RPCs currently awaited
  let scopeErrorShown = null; // the terminal scope error currently rendered
  let fileMenuPath = null; // the file whose header ⋯ is open
  const noiseExpanded = new Set(); // changesets whose collapsed noise group is open
  // The triage overlay's reviewer-owned state: which collapsed groups they have
  // opened (keyed by changeset, so opening one on the uncommitted stack says
  // nothing about a commit's), and whether they have dialled the ordering off
  // for this project.
  const expandedGroups = new Map(); // changeset key → the group names opened in it
  const triageProject = projectId || (scope && scope.project_id) || null;
  let trustDial = loadTrustDial(triageProject);
  // Re-review memory, per changeset: what the reviewer saw when they last sent
  // comments on it, so the next pass can mark what moved. renderedFiles is the
  // freshest parsed diff of the OPEN changeset, which is what a stamp is of.
  let reviewStamps = new Map();
  let renderedFiles = [];
  const branchControl = showBranchControl(scope); // interactive branch menu?
  let pendingConfirm = null; // the armed inline-confirm key (discard/force/abort/delete)
  let armedAt = null; // Date.now() when pendingConfirm was armed (for TTL expiry)
  let branchMenuOpen = false; // the branch dropdown is showing
  let branchList = null; // the last git.branches payload (null until fetched)
  const forceDeleteOffered = []; // branches whose non-force delete failed → offer force
  // One input drives the menu: it filters the list fuzzily AND names the branch
  // the "Create …" row would cut. Survives repaints.
  let branchQuery = "";
  let branchActive = 0; // keyboard cursor over the menu's rows
  let drawer = null; // the rail's narrow-viewport pull-out, re-wired per skeleton

  container.innerHTML = '<div class="gitpane"><div class="empty">loading…</div></div>';

  const messageBox = () => container.querySelector(".gitmsg");
  const draftBusy = () => {
    const box = messageBox();
    return Boolean(box && (box.value.trim() || document.activeElement === box));
  };
  const setHint = (text) => {
    hint = text || "";
    container.querySelectorAll(".githint").forEach((el) => (el.textContent = hint));
  };
  const actionError = (e) => setHint("error: " + ((e && e.message) || "error").slice(0, 70));

  // ---- the ONE inline-confirm arm/disarm path (force push / discard / abort /
  // branch delete). Every armed confirm is stamped so the poll can auto-expire
  // it, and any other action disarms it — no verb keeps its own bookkeeping.
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
          <div class="gp-banner"></div>
          <div class="cdetail-host"></div>
          <div class="gp-commit"></div>
        </section>
      </div></div>`;
    // The rail is the drawer on a narrow viewport. Both kinds of row it holds —
    // a set of changes, a commit — put something in the detail column behind
    // it, so both close it; the "show more" row, which only lengthens the rail,
    // does not.
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

  const defaultSelection = () => defaultChangesSelection({ status: lastStatus });

  // Disagreeing with the pass. Only a run has a pass to disagree with (and a
  // run_id to name in the RPC), so a bare worktree or the primary checkout
  // mounts none and its stack draws no offers.
  const overrides = commentsSupported(scope)
    ? createTriageOverrides({
        post: ({ hunk_id, direction, note }) =>
          callRpc("triage.override", { run_id: scope.run_id, hunk_id, direction, note }),
        onChange: () => render(),
      })
    : null;

  /** The pass as the reviewer's latest word makes it: what this pane renders,
   *  and what it decides to repaint on. */
  const currentTriage = () => (overrides ? overrides.apply(triage()) : triage());

  /** The freeze key for one poll: the repo's own, plus what the triage overlay
   *  is drawing from. A pass landing (or a re-pass reclassifying) moves nothing
   *  in git, so without it the ordering would wait for the next commit. */
  const pollKeyNow = (status, log) => [gitPollKey(status, log), triageFingerprint(currentTriage())].join("\x03");

  // Comments are a conversation post, so they exist where there is an agent to
  // post to. One layer serves every changeset: switching selection keeps the
  // pending set (they name their own files), and the tray renders under
  // whichever changeset is open.
  const commentable = commentsSupported(scope);
  const commentLayer = commentable
    ? createCommentLayer({
        submit: async (messages) => {
          await callRpc("run.request_changes", { run_id: scope.run_id, ...agentSelection.scope(), messages });
          // Stamp what was just reviewed, per changeset: the next pass marks
          // which of ITS files moved since the comments went out.
          reviewStamps = stampChangeset(reviewStamps, selected, renderedFiles);
        },
        revisionId,
        onChange: () => render(),
      })
    : null;

  /** The triage overlay for the stack being drawn, or null on a surface that
   *  has no pass to read: only a run is triaged, so a bare worktree or the
   *  primary checkout renders the plain stack it always did. The pass is read
   *  fresh on every paint — a re-triage lands under this pane while it is open. */
  const triageOverlay = (patch) => {
    if (!commentsSupported(scope)) return null;
    return {
      triage: currentTriage(),
      patch: patch || "",
      dial: trustDial,
      expandedGroups: expandedGroups.get(String(selected)) || null,
      // The offers are on the hunks of the changeset the pass actually read;
      // a hunk it never named renders none (core/triageModel).
      overridable: Boolean(overrides),
    };
  };

  /** The one renderer for every changeset: a header, the stacked full file
   *  diffs (noise collapsed into its group at the bottom), and — where the
   *  surface can talk to an agent — the pending-comment tray. */
  const renderChangeset = (detailHost) => {
    // Every stack carries the same re-review chip: a file that moved since the
    // reviewer last sent comments on THIS changeset says so.
    const stackFor = (files, patch) => ({
      commentable,
      noiseExpanded: noiseExpanded.has(String(selected)),
      changedSince: changedSinceChangeset(reviewStamps, selected, files),
      // Review prioritization, on the changeset the reviewer has open — the
      // rail is never reordered, only the stack under it. A surface with no run
      // behind it has no pass to read and takes the plain stack.
      review: triageOverlay(patch),
    });
    if (selected === "uncommitted") {
      if (!hasUncommittedChanges(lastStatus)) {
        renderedFiles = [];
        detailHost.innerHTML = uncommittedHeaderHtml(lastStatus) + changesetPlaceholderHtml("No uncommitted changes.");
        return;
      }
      const files = parseDiff(lastStatus.patch);
      renderedFiles = files;
      // The file's own destructive verb lives behind the header ⋯ — the stage
      // checkboxes it replaced are gone with the staged set.
      const fileMenu = supportsRepoManagement(lastStatus) ? { openPath: fileMenuPath, pendingConfirm } : null;
      detailHost.innerHTML =
        uncommittedHeaderHtml(lastStatus) +
        diffStackHtml(files, { ...stackFor(files, lastStatus.patch), fileMenu }) +
        (commentLayer ? commentLayer.trayHtml() : "");
      if (commentLayer) commentLayer.attach(detailHost);
      return;
    }
    const detail = showCache.get(selected);
    if (!detail) {
      renderedFiles = [];
      detailHost.innerHTML = '<div class="empty cdetail-loading">loading…</div>';
      return;
    }
    const commitFiles = parseDiff(detail.patch);
    renderedFiles = commitFiles;
    detailHost.innerHTML =
      commitHeaderHtml(detail) +
      diffStackHtml(commitFiles, stackFor(commitFiles, detail.patch)) +
      (commentLayer ? commentLayer.trayHtml() : "");
    if (commentLayer) commentLayer.attach(detailHost);
  };

  /** The commit box: disclosed only while uncommitted changes exist, and only
   *  on the changeset it commits. It commits everything — the message is the
   *  only input it takes. */
  const renderCommitBox = () => {
    const commitHost = container.querySelector(".gp-commit");
    if (!commitHost) return;
    const box = messageBox();
    // A live box is the freshest draft; otherwise (first paint after a
    // dispose/remount) the module-level stash restores what was typed.
    const draft = resolveCommitDraft(box ? box.value : null, commitDraftStash, draftKey);
    const hadFocus = box && document.activeElement === box;
    if (!(selected === "uncommitted" && commitBoxVisible(lastStatus))) {
      commitHost.innerHTML = "";
      syncCommitDraft(commitDraftStash, draftKey, draft);
      return;
    }
    commitHost.innerHTML = commitBoxHtml();
    const freshBox = messageBox();
    if (freshBox) {
      freshBox.value = draft;
      // Every keystroke lands in the stash so tab switches and view-shell
      // rebuilds (which remount the pane from scratch) restore the draft.
      freshBox.oninput = () => syncCommitDraft(commitDraftStash, draftKey, freshBox.value);
      if (hadFocus) freshBox.focus();
    }
    syncCommitDraft(commitDraftStash, draftKey, draft);
    const actionsHost = commitHost.querySelector(".gitcommit-actions");
    if (actionsHost) {
      mountSplitButton(actionsHost, { options: commitSplitOptions(agentCommitOptions), run: runCommitOption });
      // A repaint during an in-flight action must not resurrect an enabled
      // commit button (double-fire) — remount it disabled until the RPC settles.
      if (inFlightActions > 0) {
        const primaryButton = actionsHost.querySelector(".btn.primary:not(.caret)");
        if (primaryButton) primaryButton.disabled = true;
      }
    }
  };

  const render = () => {
    if (disposed || !lastStatus || !lastLog) return;
    if (!container.querySelector(".changes2")) paintSkeleton();
    const repoControls = supportsRepoManagement(lastStatus);
    const branchMarkup = repoControls
      ? gitBranchControlHtml({
          branch: lastStatus.branch,
          showBranchControl: branchControl,
          branchMenuHtml: branchMenuOpen
            ? branchMenuHtml(branchList, {
                pendingConfirm,
                forceDeleteOffered,
                query: branchQuery,
                activeIndex: branchActive,
              })
            : "",
        })
      : "";
    container.querySelector(".gp-toolbar").innerHTML = repoControls
      ? gitToolbarHtml({
          chips: syncChipState(lastStatus),
        })
      : "";
    container.querySelector(".gp-banner").innerHTML = repoControls
      ? gitStateBannerHtml(repoStateBanner(lastStatus.repo_state), { pendingConfirm })
      : "";
    const mergedLog = {
      ...lastLog,
      commits: [...(lastLog.commits || []), ...extraCommits],
      more: pagedMore ?? lastLog.more,
    };
    paintRail({
      review: review ? { base: review.getBase() } : null,
      status: lastStatus,
      log: mergedLog,
      selected,
      branchControlHtml: branchMarkup,
    });
    const detailHost = container.querySelector(".cdetail-host");
    if (selected === "review") {
      // The plug owns the detail DOM — mount once, then leave it alone.
      if (!reviewMounted) {
        detailHost.innerHTML = "";
        review.mount(detailHost);
        reviewMounted = true;
      }
    } else {
      if (reviewMounted) {
        review.unmount();
        reviewMounted = false;
      }
      if (selected === null || selected === undefined) {
        // A clean branch opens at the commit list: nothing selected, no commit
        // box, and a line saying what to do rather than an empty pane.
        detailHost.innerHTML = changesetPlaceholderHtml("Pick a commit to see what changed.");
      } else {
        renderChangeset(detailHost);
      }
    }
    renderCommitBox();
    setHint(hint);
    mountToolbarControls();
    // Every toolbar verb stays disabled through an in-flight action (a repaint
    // mid-action must not resurrect a live button to double-fire); the action
    // wrapper re-enables them once the RPC settles.
    if (toolbarControlsDisabled(inFlightActions)) disableToolbarControls();
    wireBranchMenu();
  };

  /// The rail, reconciled row by row rather than rewritten.
  ///
  /// Every row it holds has a name — "uncommitted", a commit's sha, the labels
  /// and affordances between them — so a poll that added one commit inserts one
  /// row, and the rest of the rail, its scroll and the branch menu open over it
  /// are exactly where they were. Nothing in the rail is wired to a row: the
  /// surface's one click handler reads which row was pressed off the DOM.
  const paintRail = (parts) => {
    const host = container.querySelector(".crail-host");
    const rail = host.querySelector(".crail") || host.appendChild(el('<div class="crail"></div>'));
    patchList(rail, changesRailEntries(parts), { keyOf: (entry) => entry.key, render: (entry) => entry.html });
  };

  /** The branch menu's input + placement. Typing re-renders the menu (the filter
   *  IS the list), Enter takes the first row, and the menu is positioned in
   *  viewport coordinates from the button — it lives inside the rail, which
   *  scrolls, and an absolutely-positioned menu was clipped on both sides along
   *  with the shadow that made it read as a layer. */
  const wireBranchMenu = () => {
    const menu = container.querySelector(".gtbranch-menu");
    const button = container.querySelector(".gtbranchbtn");
    if (!menu || !button) return;
    if (button.getBoundingClientRect) {
      const box = button.getBoundingClientRect();
      menu.style.left = `${box.left}px`;
      menu.style.top = `${box.bottom + 4}px`;
      menu.style.minWidth = `${Math.max(box.width, 260)}px`;
    }
    // Keep the cursor on a row that still exists after a filter narrowed the list.
    const rows = [...menu.querySelectorAll(".gtbranch-item")];
    if (branchActive >= rows.length) branchActive = 0;
    const activeRow = rows[branchActive];
    if (activeRow && activeRow.scrollIntoView) activeRow.scrollIntoView({ block: "nearest" });

    const input = menu.querySelector(".gtbranch-newinput");
    if (!input) return;
    input.oninput = () => {
      branchQuery = input.value;
      branchActive = 0; // a new query is a new list; start at its best match
      render();
      // The repaint replaces the input, so put the caret back where it was.
      const fresh = container.querySelector(".gtbranch-newinput");
      if (fresh) {
        fresh.focus();
        fresh.setSelectionRange(fresh.value.length, fresh.value.length);
      }
    };
    // The whole menu is driven from this input: it never loses focus, so ↑/↓ move
    // a cursor through the rows, Enter takes the one under it, and Esc gives up.
    input.onkeydown = (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        resetBranchMenu();
        render();
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        branchActive = moveActiveIndex(branchActive, event.key === "ArrowDown" ? 1 : -1, rows.length);
        render();
        return;
      }
      if (event.key !== "Enter") return;
      event.preventDefault();
      const chosen = rows[branchActive];
      if (!chosen) return;
      if (chosen.classList.contains("gtbranch-create")) createBranch();
      else if (inFlightActions === 0) checkoutBranch(chosen.dataset.branch);
    };
    if (document.activeElement !== input) input.focus();
  };

  /** Mount the Pull/Push/Stash split buttons into their toolbar hosts. Each host
   *  is absent unless the repo-management toolbar rendered (older bridge → no
   *  hosts, nothing to mount). The git bar is a dense toolbar, so its verbs wear
   *  the mini button — the same one Fetch and the branch control wear. */
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

  const toolbarButtons = () => [
    ...container.querySelectorAll(".gtbranchbtn"),
    ...container.querySelectorAll(".gtsync .btn, .gtstash .btn"),
  ];
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
    render();
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
  const runCommitOption = async (optionId) => {
    inFlightActions += 1;
    try {
      return await performCommitOption(optionId);
    } finally {
      settleInFlight();
    }
  };

  const performCommitOption = async (optionId) => {
    setHint("");
    if (optionId === "commit") {
      const box = messageBox();
      const message = box ? box.value.trim() : "";
      if (!message) {
        setHint("Enter a commit message first.");
        throw new Error("commit message must not be empty");
      }
      // Commit is commit-all: there is no staged set to assemble, so every
      // changed path is staged first and the commit takes the lot.
      const paths = commitAllPaths(lastStatus);
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
      try {
        await callRpc("run.message", { run_id: scope.run_id, message: AGENT_COMMIT_MESSAGE });
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
        await callRpc("run.git_action", { run_id: scope.run_id, action: "commit" });
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
    clearConfirm();
    fileMenuPath = null; // a menu belongs to the changeset it was opened on
    render();
    if (sel !== "review" && sel !== "uncommitted" && !showCache.has(sel)) fetchShow(sel);
  };

  const fetchShow = async (hash) => {
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

  // ---- repo-management actions (v2 toolbar / banner / discard / branch) ----

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

  const resetBranchMenu = () => {
    branchMenuOpen = false;
    branchList = null;
    branchQuery = "";
    branchActive = 0;
    forceDeleteOffered.length = 0;
    clearConfirm();
  };

  const closeBranchMenu = () => {
    resetBranchMenu();
    render();
  };

  /** Open the branch dropdown and load git.branches. Every branch RPC carries
   *  the whole scope: on a worktree surface the switch belongs to THAT
   *  checkout, and sending the project id alone would move the project's. A
   *  second click on the branch button closes it. */
  const toggleBranchMenu = async () => {
    if (branchMenuOpen) {
      closeBranchMenu();
      return;
    }
    branchMenuOpen = true;
    branchList = null;
    clearConfirm();
    render(); // the loading placeholder shows immediately
    let payload;
    try {
      payload = await callRpc("git.branches", { ...scope });
    } catch (e) {
      if (!disposed) actionError(e);
      branchMenuOpen = false;
      render();
      return;
    }
    if (disposed || !branchMenuOpen) return;
    branchList = payload;
    render();
  };

  const checkoutBranch = (branch, create = false) =>
    runGuarded(async () => {
      let status;
      try {
        status = await callRpc("git.checkout", { ...scope, branch, create });
      } catch (e) {
        if (!disposed) actionError(e);
        return;
      }
      resetBranchMenu();
      await applyStatusResult(status, create ? `Created ${branch}.` : `Switched to ${branch}.`);
    });

  const createBranch = () => {
    const name = (branchQuery || "").trim();
    if (!name) {
      setHint("Type a branch name first.");
      return undefined;
    }
    return checkoutBranch(name, true);
  };

  const deleteBranch = (branch, force) =>
    runGuarded(async () => {
      let payload;
      try {
        payload = await callRpc("git.branch_delete", { ...scope, branch, force });
      } catch (e) {
        if (!disposed) {
          actionError(e);
          // Offer a force delete only after a non-force delete has failed.
          if (!force && !forceDeleteOffered.includes(branch)) forceDeleteOffered.push(branch);
          clearConfirm();
          render();
        }
        return;
      }
      if (disposed) return;
      branchList = payload; // git.branch_delete returns the fresh branches list
      const offered = forceDeleteOffered.indexOf(branch);
      if (offered >= 0) forceDeleteOffered.splice(offered, 1);
      clearConfirm();
      setHint(`Deleted ${branch}.`);
      render(); // the menu stays open, now without the deleted branch
    });

  /** The inline-confirm gate for a destructive click: a first click arms and
   *  repaints the armed label; a second on the same control fires `action`. */
  const confirmThen = (key, action) => {
    if (inFlightActions > 0) return;
    if (armConfirm(key)) action();
    else render();
  };

  const handleClick = (event) => {
    const target = event.target;
    if (toggleSecretSpoiler(target)) return; // reveal/hide a masked dotenv value in a diff
    if (target.closest(".gtfetch")) {
      runFetch();
      return;
    }
    // The Pull/Push/Stash split buttons wire their own behavior (including the
    // force-push inline confirm inside runSyncOption). Their menu-item clicks
    // bubble here, so bail before the "disarm on any other click" fallthrough —
    // otherwise a click would clear the very confirm it just armed.
    if (target.closest(".gtsync") || target.closest(".gtstash")) return;
    if (target.closest(".gtbranchbtn")) {
      toggleBranchMenu();
      return;
    }
    if (branchMenuOpen) {
      const deleteButton = target.closest(".gtbranch-del");
      if (deleteButton) {
        const branch = deleteButton.dataset.branch;
        const force = deleteButton.dataset.force === "1";
        confirmThen(`${force ? "branch_delete_force" : "branch_delete"}:${branch}`, () => deleteBranch(branch, force));
        return;
      }
      const item = target.closest(".gtbranch-item");
      if (item) {
        // The create row wears the item class so it lands in the same list — it
        // is an answer to the query, not a separate form.
        if (item.classList.contains("gtbranch-create")) {
          createBranch();
          return;
        }
        // Branch rows are <div>s, so (unlike the disabled toolbar buttons) they
        // stay clickable during an in-flight action — guard the checkout here.
        if (inFlightActions === 0) checkoutBranch(item.dataset.branch);
        return;
      }
      if (target.closest(".gtbranch-menu")) return; // a click on the input keeps the menu open
      closeBranchMenu(); // any other click dismisses the menu, then falls through
    }
    const abortButton = target.closest(".gitabort");
    if (abortButton) {
      confirmThen("abort", runAbort);
      return;
    }
    const discardButton = target.closest(".gitdiscard");
    if (discardButton) {
      confirmThen(`discard:${discardButton.dataset.path}`, () => runDiscard(discardButton.dataset.path));
      return;
    }
    // The file header's ⋯ — where the per-file verbs live now that the stage
    // checkboxes are gone. One menu is open at a time; a second click shuts it.
    const menuButton = target.closest(".fmenu");
    if (menuButton) {
      const path = menuButton.dataset.path;
      fileMenuPath = fileMenuPath === path ? null : path;
      clearConfirm();
      render();
      return;
    }
    // Disagreeing with where the pass put a hunk. This runs BEFORE the comment
    // and fold handling: the offer sits on a hunk row inside a capped file, and
    // either would otherwise eat the press as "expand me" or "comment here".
    // While the review plug owns the detail pane it owns its overlay too — this
    // layer must not also claim it, or one press would post two disagreements.
    if (!reviewMounted && overrides && overrides.handleClick(event)) return;
    // The trust dial: the reviewer says how much of the pass's reading they
    // want. Remembered per project, so the answer is asked once.
    if (target.closest(".tdial")) {
      trustDial = !trustDial;
      saveTrustDial(triageProject, trustDial);
      render();
      return;
    }
    // A collapsed triage group: a click opens it, per changeset, across
    // repaints — the same discipline the noise group is opened with.
    const groupHead = target.closest(".tgrouphead");
    if (groupHead) {
      const key = String(selected);
      if (!expandedGroups.has(key)) expandedGroups.set(key, new Set());
      const opened = expandedGroups.get(key);
      const name = groupHead.dataset.group;
      if (opened.has(name)) opened.delete(name);
      else opened.add(name);
      render();
      return;
    }
    // The collapsed noise group at the bottom of a stack: a click opens it (and
    // the choice sticks per changeset across repaints).
    if (target.closest(".noisehead")) {
      const key = String(selected);
      if (noiseExpanded.has(key)) noiseExpanded.delete(key);
      else noiseExpanded.add(key);
      render();
      return;
    }
    // Rail selection: the pinned entries and the commit rows. selectRail
    // clears any armed confirm itself.
    const railRow = target.closest(".rrow[data-sel]");
    if (railRow) {
      selectRail(railRow.dataset.sel);
      return;
    }
    const commitRow = target.closest(".crow[data-hash]");
    if (commitRow) {
      selectRail(commitRow.dataset.hash);
      return;
    }
    // The comment affordances every changeset carries: ✎ on a file header, the
    // tray's remove control, and a tap on a line of an expanded file. This runs
    // BEFORE the fold handling: ✎ sits inside a capped file's header, and the
    // fold handler would otherwise eat the click as "expand me". While the
    // review plug owns the detail pane it owns its comments too — this layer
    // must not also claim them, or one tap would write two comments.
    if (!reviewMounted && commentLayer && commentLayer.handleClick(event)) return;
    // Diff folding, shared by every detail (uncommitted, commit, review plug):
    // the filename bar toggles a full collapse; a click on a capped body
    // expands it. Controls in the bar (⋯, ✎) keep their jobs.
    const fhead = target.closest(".fhead");
    if (fhead && !target.closest("button, input, label")) {
      const file = fhead.closest(".file");
      if (file) {
        file.classList.toggle("collapsed");
        file.classList.remove("capped");
        return;
      }
    }
    const cappedFile = target.closest(".file.capped");
    if (cappedFile) {
      cappedFile.classList.remove("capped");
      return;
    }
    // Any other click disarms a stale confirm before doing its own job.
    if (disarmConfirm()) render();
    if (target.closest(".gitmore")) {
      showMore();
      return;
    }
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

  const poll = async () => {
    if (disposed) return;
    let status, log;
    try {
      [status, log] = await Promise.all([callRpc("git.status", { ...scope }), callRpc("git.log", { ...scope })]);
    } catch (e) {
      // Permanent scope errors (pruned task, removed project) never recover —
      // surface them instead of "loading…" forever; everything else is
      // transient and the poll retries silently.
      if (!disposed && isPermanentGitScopeError(e && e.message)) renderScopeError((e && e.message) || "error");
      return;
    }
    if (disposed) return;
    scopeErrorShown = null; // recovered — the next render paints normally
    if (lastHead !== undefined && status.head !== lastHead) {
      extraCommits = []; // HEAD moved — the paged-in history is stale
      pagedMore = null;
    }
    lastHead = status.head;
    lastStatus = status;
    lastLog = log;
    // Where the surface opens is a function of the tree: the first status picks
    // it, and an empty selection follows the tree into dirt afterwards.
    selected = selected === undefined ? defaultSelection() : selectionAfterPoll(selected, status);
    // An abandoned confirm auto-expires: past the TTL the poll disarms it and
    // forces a repaint (S2c), so a destructive verb never stays one click from
    // firing — and the interactionActive freeze it caused is released too.
    const expired = confirmExpired(armedAt, Date.now());
    if (expired) clearConfirm();
    const key = pollKeyNow(status, log);
    const rendered = container.querySelector(".gitpane .changes2");
    // Freeze while unchanged, while the user is drafting a commit message, while
    // any action RPC is in flight, or while an interaction is live: an armed
    // confirm, an open branch or file menu, or a review in progress (pending
    // comments, an open popover, typed general text) a repaint would clobber. A
    // just-expired confirm bypasses the freeze so its armed label actually clears.
    if (
      !expired &&
      pollRenderFrozen({
        paneRendered: Boolean(rendered),
        keyUnchanged: key === renderedKey,
        draftActive: draftBusy(),
        actionInFlight: inFlightActions > 0,
        interactionActive:
          Boolean(pendingConfirm) ||
          branchMenuOpen ||
          Boolean(branchQuery) ||
          fileMenuPath !== null ||
          // A split button's menu (Commit, Pull, Push, Stash) is open because
          // somebody is reaching into it, and the repaint that rebuilds the
          // toolbar would shut it. It closes itself on any press outside, so
          // this can never hold the poll for longer than the reach.
          Boolean(container.querySelector(".splitmenu:not([hidden])")) ||
          Boolean(commentLayer && commentLayer.busy()),
      })
    )
      return;
    renderedKey = key;
    render();
  };

  // A press anywhere outside the pane dismisses a live interaction (open branch
  // menu / armed confirm), mirroring splitButton's own outside-close. Without it
  // an abandoned menu/confirm keeps interactionActive true and freezes the poll
  // until the user clicks back inside (S5). Removed on dispose.
  const onOutsidePointerDown = (event) => {
    if (
      !outsidePressDismisses({
        inside: container.contains(event.target),
        branchMenuOpen,
        hasPendingConfirm: pendingConfirm !== null,
        fileMenuOpen: fileMenuPath !== null,
      })
    )
      return;
    if (branchMenuOpen) resetBranchMenu();
    else clearConfirm();
    fileMenuPath = null; // an abandoned file menu must not freeze the poll
    render();
  };
  document.addEventListener("pointerdown", onOutsidePointerDown);

  poll();
  // The pane reads one checkout, so it refetches when that checkout's entity
  // moves — the git watcher stales a run the instant files land in it. A
  // project's own checkout is not an entity the bridge names, and its state
  // moves with the feed, so that scope watches the board instead.
  const watcher = watchChanges({
    refresh: poll,
    intervalMs: GIT_PANE_POLL_MS,
    entity: scope.run_id || scope.worktree_id || null,
  });

  return {
    dispose() {
      disposed = true;
      watcher.dispose();
      document.removeEventListener("pointerdown", onOutsidePointerDown);
      if (reviewMounted) {
        review.unmount(); // stop the plug's poll; the view may remount it later
        reviewMounted = false;
      }
      if (drawer) {
        drawer.dispose();
        drawer = null;
      }
      if (commentLayer) commentLayer.dispose();
      if (overrides) overrides.dispose();
      container.onclick = null;
    },
  };
}
