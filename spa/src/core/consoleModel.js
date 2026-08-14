// The console's pure model.
//
// The console is the bottom of the view column: a shut bar by default, a half
// panel, or an overlay over the whole work surface. What it holds is the
// terminals of the checkout the selected work item stands in — a branch's
// worktree, or the primary checkout for an issue and for main. This module
// answers the questions that have nothing to do with the DOM: how big it is,
// how big it should be next, which directory its terminals live in, whether a
// keystroke belongs to it, and which terminal a pre-redesign URL asked for.

/** Shut, half the view, or over all of it. The order is the order of growth. */
export const CONSOLE_SIZES = ["collapsed", "half", "full"];

const SIZE_KEY_PREFIX = "build.console.size.";

/** A stored or passed-in size, or the shut bar — which is what an unreadable
 *  memory and a first visit both mean. */
export function consoleSize(value) {
  return CONSOLE_SIZES.includes(value) ? value : "collapsed";
}

/** The work item whose console this is. Each branch and each issue keeps its
 *  own — the terminals are the checkout's, so the size belongs to it too. */
export function consoleKey(context) {
  if (!context) return "none";
  return context.kind === "issue" ? `issue:${context.issueId}` : `branch:${context.projectId}:${context.branch}`;
}

/** What this device last chose for that work item. */
export function readConsoleSize(key, storage = globalThis.localStorage) {
  try {
    return consoleSize(storage.getItem(SIZE_KEY_PREFIX + key));
  } catch {
    return "collapsed"; // private mode: the choice lasts the mount
  }
}

export function writeConsoleSize(key, size, storage = globalThis.localStorage) {
  try {
    storage.setItem(SIZE_KEY_PREFIX + key, consoleSize(size));
  } catch {
    /* private mode: the choice lasts the mount */
  }
}

/** The caret, and the backtick: shut it, or put it back at half. A full overlay
 *  shuts — the way out of covering the work is to stop covering it. */
export function toggledConsoleSize(size) {
  return consoleSize(size) === "collapsed" ? "half" : "collapsed";
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
 * `row` is the branch's `branch.get` payload; an issue needs none — its agent
 * runs on the primary checkout, which the project alone names.
 */
export function consoleScope(context, row) {
  if (!context) return null;
  if (context.kind === "issue") return context.projectId ? { project_id: context.projectId } : null;
  if (!row) return null;
  if (row.run_id) return { run_id: row.run_id };
  const projectId = row.project_id || context.projectId;
  if (!projectId) return null;
  if (row.worktree_id) return { project_id: projectId, worktree_id: row.worktree_id };
  // A branch row with neither is the repository itself: main, in the checkout
  // every project is cloned into.
  return row.primary ? { project_id: projectId } : null;
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
