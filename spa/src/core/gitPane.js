// DOM-light controller for the git surface (the Changes tab): owns a 1.6s
// git.status + git.log poll with a keyed freeze, per-file stage/unstage
// checkboxes (repainted from the RPC's returned status), commit-row expansion
// (git.show, cached per hash), "Show more" paging via skip, and the commit
// split button. The pure helpers (poll key, option lists) are exported for
// unit tests; mountGitPane is the only DOM-touching entry point.

import { uncommittedHtml, historyHtml, AGENT_COMMIT_MESSAGE } from "./gitRender.js";
import { mountSplitButton } from "./splitButton.js";

export const GIT_PANE_POLL_MS = 1600;

/** The repaint-freeze key for one poll's payloads: HEAD + branch + the status
 *  patch + every file's stage state + the visible commit page. Unchanged key →
 *  the poll leaves the DOM (and the user's checkbox focus) alone. */
export function gitPollKey(status, log) {
  const files = (status.files || [])
    .map((f) => [f.path, f.staged, f.index_status, f.worktree_status].join(""))
    .join("");
  const commits = ((log && log.commits) || []).map((c) => c.hash).join(",");
  return [status.branch, status.head, status.truncated, status.patch, files, commits, Boolean(log && log.more)].join("");
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
    const draft = box ? box.value : "";
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
      if (hadFocus) freshBox.focus();
    }
    // `indeterminate` is a property, not an attribute — set it after mount.
    const filesByPath = new Map((lastStatus.files || []).map((f) => [f.path, f]));
    container.querySelectorAll(".stagebox").forEach((checkbox) => {
      const file = filesByPath.get(checkbox.dataset.path);
      if (file && file.staged === "partial") checkbox.indeterminate = true;
    });
    setHint(hint);
    const actionsHost = container.querySelector(".gitcommit-actions");
    if (actionsHost) mountSplitButton(actionsHost, { options: commitSplitOptions(agentCommitOptions), run: runCommitOption });
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

  const runCommitOption = async (optionId) => {
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
      if (box) box.value = "";
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
      await forceRefresh();
      return;
    }
    throw new Error(`unknown commit option ${optionId}`);
  };

  const toggleStaged = async (checkbox) => {
    const path = checkbox.dataset.path;
    const method = checkbox.checked ? "git.stage" : "git.unstage";
    checkbox.disabled = true;
    let status;
    try {
      status = await callRpc(method, { ...scope, paths: [path] });
    } catch (e) {
      if (disposed) return;
      checkbox.disabled = false;
      checkbox.checked = !checkbox.checked;
      actionError(e);
      return;
    }
    if (disposed) return;
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

  const poll = async () => {
    if (disposed) return;
    let status, log;
    try {
      [status, log] = await Promise.all([callRpc("git.status", { ...scope }), callRpc("git.log", { ...scope })]);
    } catch {
      return; // transient — the poll retries silently
    }
    if (disposed) return;
    if (lastHead !== undefined && status.head !== lastHead) {
      extraCommits = []; // HEAD moved — the paged-in history is stale
      pagedMore = null;
    }
    lastHead = status.head;
    lastStatus = status;
    lastLog = log;
    const key = gitPollKey(status, log);
    const rendered = container.querySelector(".gitpane .gitsec");
    // Freeze while unchanged, or while the user is drafting a commit message.
    if (rendered && (key === renderedKey || draftBusy())) return;
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
