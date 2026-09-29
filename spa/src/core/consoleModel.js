// The console's pure model.
//
// The console is the bottom of the view column: a shut bar by default, a half
// panel, or an overlay over the whole work surface. What it holds is the
// terminals of the checkout the selected work item stands in — a workspace's
// root, a branch's worktree, or the project's own directory. This module
// answers the questions that have nothing to do with the DOM: how big it is,
// how big it should be next, which directory its terminals live in, whether a
// keystroke belongs to it, and which terminal a pre-redesign URL asked for.

import { workspaceKey } from "./deviceKey.js";

/** Shut, half the view, or over all of it. The order is the order of growth. */
export const CONSOLE_SIZES = ["collapsed", "half", "full"];

const SIZE_KEY_PREFIX = "build.console.size.";
const REOPEN_KEY_PREFIX = "build.console.reopen.";

export const DEFAULT_OPEN_SIZE = "half";

/** A stored or passed-in size, or the shut bar — which is what an unreadable
 *  memory and a first visit both mean. */
export function consoleSize(value) {
  return CONSOLE_SIZES.includes(value) ? value : "collapsed";
}

/** The work item whose console this is. Each workspace, branch and task keeps
 *  its own — the terminals are the checkout's, so the size belongs to it too.
 *  A workspace is one machine's, and a workspace id is one bridge's, so its key
 *  carries the machine (core/deviceKey.js). */
export function consoleKey(context) {
  if (!context) return "none";
  if (context.kind === "workspace") return `workspace:${workspaceKey(context.deviceId, context.workspaceId)}`;
  return context.kind === "task" ? `task:${context.taskId}` : `branch:${context.projectId}:${context.branch}`;
}

/** What this device last chose for that work item. */
export function readConsoleSize(key, storage = globalThis.localStorage) {
  try {
    return consoleSize(storage.getItem(SIZE_KEY_PREFIX + key));
  } catch {
    return "collapsed"; // private mode: the choice lasts the mount
  }
}

function rememberSize(prefix, key, size, storage) {
  try {
    storage.setItem(prefix + key, consoleSize(size));
  } catch {
    /* private mode: the choice lasts the mount */
  }
}

export function writeConsoleSize(key, size, storage = globalThis.localStorage) {
  rememberSize(SIZE_KEY_PREFIX, key, size, storage);
}

const openSizeOf = (value) => (consoleSize(value) === "collapsed" ? DEFAULT_OPEN_SIZE : consoleSize(value));

export function toggledConsoleSize(size, openSize = DEFAULT_OPEN_SIZE) {
  return consoleSize(size) === "collapsed" ? openSizeOf(openSize) : "collapsed";
}

export function readConsoleReopenSize(key, storage = globalThis.localStorage) {
  try {
    return openSizeOf(storage.getItem(REOPEN_KEY_PREFIX + key));
  } catch {
    return DEFAULT_OPEN_SIZE;
  }
}

export function writeConsoleReopenSize(key, size, storage = globalThis.localStorage) {
  if (consoleSize(size) === "collapsed") return;
  rememberSize(REOPEN_KEY_PREFIX, key, size, storage);
}

/** The grow control: bigger, until there is no bigger, and then back. */
export function grownConsoleSize(size) {
  return consoleSize(size) === "full" ? "half" : "full";
}

/**
 * The terminal scope of the checkout this work item stands in — the shapes
 * `term.list`/`term.create` take (`{run_id}`, `{project_id, worktree_id}`,
 * `{project_id}`), or null when nothing here names a directory.
 *
 * `row` is the branch's row (views/branchSeed.js); a task needs none — its agent
 * runs in the project's own checkout, which the project alone names.
 */
const scopeResolvers = {
  // A workspace owns its terminals as a whole. Its selected source directory
  // deliberately does not participate in terminal identity.
  workspace: (context) => context.workspaceId ? { workspace_id: context.workspaceId } : null,
  task: (context) => context.projectId ? { project_id: context.projectId } : null,
  branch: (context, row) => branchConsoleScope(context, row),
};

function branchConsoleScope(context, row) {
  if (!row) return null;
  if (row.run_id) return { run_id: row.run_id };
  const projectId = row.project_id || context.projectId;
  if (!projectId) return null;
  if (row.worktree_id) return { project_id: projectId, worktree_id: row.worktree_id };
  // A row with neither is the project's own directory — the repository this
  // branch is checked out in, or a plain folder with no git in it. The project
  // alone names it.
  return { project_id: projectId };
}

export function consoleScope(context, row) {
  return scopeResolvers[context?.kind]?.(context, row) || null;
}

/**
 * The route this work item is named by, as the cache resolves routes
 * (core/cachedRows.js). The console is mounted on an address, not on an
 * entity — a branch is its project and its name, a workspace the conversation
 * it holds — and the cached rows are what turn one into the other.
 */
export function consoleFeedRoute(context) {
  if (!context) return null;
  if (context.kind === "task")
    return { name: "task", deviceId: context.deviceId, projectId: context.projectId, id: context.taskId };
  if (context.kind === "workspace")
    return {
      name: "workspace",
      deviceId: context.deviceId,
      projectId: context.projectId,
      workspaceId: context.workspaceId,
    };
  return { name: "branch", deviceId: context.deviceId, projectId: context.projectId, branch: context.branch };
}

/** Whether the backtick is the console's to take, given what has focus.
 *
 *  Anything the user is typing into keeps its own keystrokes — a text field, a
 *  terminal screen (where a backtick is a backtick), and the conversation
 *  composer beside the work. */
export function consoleTakesKey(element) {
  if (!element) return true;
  const tag = String(element.tagName || "").toUpperCase();
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return false;
  if (element.isContentEditable) return false;
  if (typeof element.closest === "function" && element.closest(".termpane, .term-screen, .thread-composer")) return false;
  return true;
}

// The terminal a pre-redesign `term-<n>` URL named. Those URLs addressed a tab
// on a surface that no longer has tabs, so the route rewrites to the branch and
// leaves the terminal here for the console that is about to mount. One-shot:
// the next console takes it, and a later one opens on whatever it remembers.
let pendingTerminal = null;

export function markConsoleTerminal(termId) {
  if (/^term-\d+$/.test(String(termId || ""))) pendingTerminal = termId;
}

export function takeConsoleTerminal() {
  const wanted = pendingTerminal;
  pendingTerminal = null;
  return wanted;
}
