// DOM-light controller for the git surface (the Changes tab): owns a 1.6s
// git.status + git.log poll with a keyed freeze, per-file stage/unstage
// checkboxes (repainted from the RPC's returned status), commit-row expansion
// (git.show, cached per hash), "Show more" paging via skip, and the commit
// split button. The pure helpers (poll key, option lists) are exported for
// unit tests; mountGitPane is the only DOM-touching entry point.

import { esc } from "./text.js";
import { uncommittedHtml, historyHtml, gitToolbarHtml, gitStateBannerHtml, branchMenuHtml, AGENT_COMMIT_MESSAGE } from "./gitRender.js";
import { mountSplitButton } from "./splitButton.js";

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

/** Branch switching is main-worktree (project scope) only — a task worktree's
 *  branch is owned by the task lifecycle, so sessions show it as static text. */
export function showBranchControl(scope) {
  return Boolean(scope && scope.project_id && !scope.task_id);
}

/** Every toolbar verb is disabled while any action RPC is in flight (the same
 *  freeze that suppresses poll repaints), so a mid-action repaint never revives
 *  a live button to double-fire. */
export function toolbarControlsDisabled(inFlightCount) {
  return inFlightCount > 0;
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
 *  (never a bare --force) gated behind an inline confirm in the controller. */
export function pushSplitOptions() {
  return [
    { id: "push", label: "Push", description: "push to the upstream", busyLabel: "Pushing…" },
    { id: "force_push", menuLabel: "Force push (with lease)", description: "overwrite remote history — safely", busyLabel: "Force pushing…", danger: true },
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

// The destructive sync-menu options that require the inline confirm before they
// fire (the primary Pull/Push/Stash/Pop actions fire immediately).
const SYNC_CONFIRM_OPTIONS = new Set(["force_push"]);

/** True when a sync option must be confirmed once before it runs. */
export function syncActionNeedsConfirm(optionId) {
  return SYNC_CONFIRM_OPTIONS.has(optionId);
}

/** The state banner for a non-clean repo, or null when clean/absent. `abortable`
 *  gates the Abort button: only merging/rebasing can be aborted (merge_abort
 *  rejects on any other state, so offering it there would only produce errors). */
export function repoStateBanner(repoState) {
  if (repoState === "merging") return { message: "Merge in progress — resolve conflicts, then commit.", abortable: true };
  if (repoState === "rebasing") return { message: "Rebase in progress — resolve conflicts, then continue.", abortable: true };
  if (repoState === "other") return { message: "Repository is in an unusual state.", abortable: false };
  return null;
}

/** The repaint-freeze key for one poll's payloads: HEAD + branch + the status
 *  patch + every file's stage state + the visible commit page. Unchanged key →
 *  the poll leaves the DOM (and the user's checkbox focus) alone. */
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
    minuteBucket,
  ].join("\x03");
}

/** The stash key for a scope's in-progress commit-message draft: drafts live in
 *  a module-level Map so tab switches and view-shell rebuilds (which remount the
 *  pane from scratch) restore them transparently. */
export function gitDraftKey(scope) {
  return scope.task_id ? `task:${scope.task_id}` : `project:${scope.project_id || ""}`;
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
  "unknown task_id",
  "provide exactly one of project_id or task_id",
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

/** The commit split-button option list: the plain Commit action first (primary),
 *  then whatever agent options the mounting view offers (task scope only). */
export function commitSplitOptions(agentCommitOptions = []) {
  return [
    { id: "commit", label: "Commit", description: "commit the staged changes with your message", busyLabel: "Committing…" },
    ...agentCommitOptions,
  ];
}

// Task states with a live/parked agent to message — the same list task.js uses
// for its "Message agent" header button.
const AGENT_MESSAGEABLE_STATES = ["planning", "building", "blocked", "failed", "idle_unreported", "interrupted"];

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
 *  one of { project_id } / { task_id }; `callRpc(method, params)` is the RPC
 *  channel; `agentCommitOptions` (task scope) appends to the commit button;
 *  `onNavigate` is reserved for future cross-surface links. */
export function mountGitPane(container, { scope, callRpc, agentCommitOptions = [], onNavigate = null } = {}) {
  void onNavigate; // accepted per the pane contract; no link targets yet
  let disposed = false;
  let renderedKey = null; // gitPollKey of the last painted payloads
  let lastStatus = null;
  let lastLog = null; // the poll's first page (limit default)
  let lastHead; // undefined until the first poll lands
  let extraCommits = []; // "Show more" pages beyond the poll's first page
  let pagedMore = null; // the last fetched page's `more` (null → use lastLog.more)
  let expandedHash = null; // survives repaints
  let hint = ""; // sticky action hint/error, re-applied after each repaint
  const showCache = new Map(); // hash → git.show payload (commits are immutable)
  const draftKey = gitDraftKey(scope); // the stash slot for this scope's draft
  let inFlightActions = 0; // commit/stage/unstage RPCs currently awaited
  const stagingPaths = new Set(); // paths whose stage/unstage RPC is in flight
  let scopeErrorShown = null; // the terminal scope error currently rendered

  container.innerHTML = '<div class="gitpane"><div class="empty">loading…</div></div>';

  const messageBox = () => container.querySelector(".gitmsg");
  const draftBusy = () => {
    const box = messageBox();
    return Boolean(box && (box.value.trim() || document.activeElement === box));
  };
  const setHint = (text) => {
    hint = text || "";
    const el = container.querySelector(".githint");
    if (el) el.textContent = hint;
  };
  const actionError = (e) => setHint("error: " + ((e && e.message) || "error").slice(0, 70));

  const render = () => {
    if (disposed || !lastStatus || !lastLog) return;
    const box = messageBox();
    // A live box is the freshest draft; otherwise (first paint after a
    // dispose/remount) the module-level stash restores what was typed.
    const draft = resolveCommitDraft(box ? box.value : null, commitDraftStash, draftKey);
    const hadFocus = box && document.activeElement === box;
    const mergedLog = {
      ...lastLog,
      commits: [...(lastLog.commits || []), ...extraCommits],
      more: pagedMore ?? lastLog.more,
    };
    const expandedDetail = expandedHash ? showCache.get(expandedHash) || null : null;
    container.innerHTML = `<div class="gitpane">${uncommittedHtml(lastStatus)}${historyHtml(mergedLog, { expandedHash, expandedDetail })}</div>`;
    const freshBox = messageBox();
    if (freshBox) {
      freshBox.value = draft;
      // Every keystroke lands in the stash so tab switches and view-shell
      // rebuilds (which remount the pane from scratch) restore the draft.
      freshBox.oninput = () => syncCommitDraft(commitDraftStash, draftKey, freshBox.value);
      if (hadFocus) freshBox.focus();
    }
    syncCommitDraft(commitDraftStash, draftKey, draft);
    // `indeterminate` is a property, not an attribute — set it after mount.
    const filesByPath = new Map((lastStatus.files || []).map((f) => [f.path, f]));
    container.querySelectorAll(".stagebox").forEach((checkbox) => {
      const file = filesByPath.get(checkbox.dataset.path);
      if (file && file.staged === "partial") checkbox.indeterminate = true;
      // A repaint must not recreate an enabled checkbox mid-stage-RPC.
      if (stagingPaths.has(checkbox.dataset.path)) checkbox.disabled = true;
    });
    setHint(hint);
    const actionsHost = container.querySelector(".gitcommit-actions");
    if (actionsHost) {
      mountSplitButton(actionsHost, { options: commitSplitOptions(agentCommitOptions), run: runCommitOption });
      // A repaint during an in-flight action must not resurrect an enabled
      // commit button (double-fire) — remount it disabled until the RPC settles.
      if (inFlightActions > 0) {
        const primaryButton = actionsHost.querySelector(".btn.primary:not(.caret)");
        if (primaryButton) primaryButton.disabled = true;
      }
    }
    container.onclick = handleClick;
    container.onchange = handleChange;
  };

  const paintFrom = (status, log) => {
    lastStatus = status;
    if (log) lastLog = log;
    renderedKey = gitPollKey(lastStatus, lastLog);
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
      inFlightActions -= 1;
      if (!disposed && inFlightActions === 0) {
        const primaryButton = container.querySelector(".gitcommit-actions .btn.primary:not(.caret)");
        if (primaryButton) primaryButton.disabled = false;
      }
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
      let result;
      try {
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
        await callRpc("task.message", { task_id: scope.task_id, message: AGENT_COMMIT_MESSAGE });
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
        await callRpc("task.git_action", { task_id: scope.task_id, action: "commit" });
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

  const stageboxFor = (path) =>
    [...container.querySelectorAll(".stagebox")].find((box) => box.dataset.path === path) || null;

  const toggleStaged = async (checkbox) => {
    const path = checkbox.dataset.path;
    const method = checkbox.checked ? "git.stage" : "git.unstage";
    checkbox.disabled = true;
    stagingPaths.add(path); // a repaint mid-RPC recreates the box disabled
    inFlightActions += 1;
    let status = null;
    try {
      status = await callRpc(method, { ...scope, paths: [path] });
    } catch (e) {
      if (!disposed) actionError(e);
    } finally {
      stagingPaths.delete(path);
      inFlightActions -= 1;
    }
    if (disposed) return;
    if (!status) {
      // Roll back: a repaint mid-RPC repainted the box from the pre-action
      // status (already correct); the original node needs its check flipped.
      const attached = stageboxFor(path);
      if (attached === checkbox) checkbox.checked = !checkbox.checked;
      if (attached) attached.disabled = false;
      return;
    }
    lastHead = status.head;
    setHint("");
    paintFrom(status); // the stage RPCs return the full status payload
  };

  const toggleExpanded = async (hash) => {
    if (expandedHash === hash) {
      expandedHash = null;
      render();
      return;
    }
    expandedHash = hash;
    render(); // cached detail paints now; otherwise the loading placeholder
    if (showCache.has(hash)) return;
    let show;
    try {
      show = await callRpc("git.show", { ...scope, hash });
    } catch (e) {
      if (disposed) return;
      if (expandedHash === hash) expandedHash = null;
      actionError(e);
      render();
      return;
    }
    showCache.set(show.hash, show);
    if (!disposed && expandedHash === hash) render();
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

  const handleClick = (event) => {
    if (event.target.closest(".gitmore")) {
      showMore();
      return;
    }
    const row = event.target.closest(".crow");
    if (row && container.contains(row)) toggleExpanded(row.dataset.hash);
  };

  const handleChange = (event) => {
    if (event.target.classList && event.target.classList.contains("stagebox")) toggleStaged(event.target);
  };

  /** A permanent scope rejection replaces the pane body (there is nothing to
   *  retry: the task/project this scope named no longer resolves). */
  const renderScopeError = (message) => {
    if (scopeErrorShown === message) return;
    scopeErrorShown = message;
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
    const key = gitPollKey(status, log);
    const rendered = container.querySelector(".gitpane .gitsec");
    // Freeze while unchanged, while the user is drafting a commit message, or
    // while any action RPC is in flight (a repaint would clobber busy state).
    if (
      pollRenderFrozen({
        paneRendered: Boolean(rendered),
        keyUnchanged: key === renderedKey,
        draftActive: draftBusy(),
        actionInFlight: inFlightActions > 0,
      })
    )
      return;
    renderedKey = key;
    render();
  };

  poll();
  const timer = setInterval(poll, GIT_PANE_POLL_MS);

  return {
    dispose() {
      disposed = true;
      clearInterval(timer);
      container.onclick = null;
      container.onchange = null;
    },
  };
}
