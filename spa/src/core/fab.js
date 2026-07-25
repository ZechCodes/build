// The floating create affordance, shown on every page inside a project.
//
// Two verbs, ranked. The primary FAB files an ISSUE — the unit of work Build
// coordinates: file it, discuss the plan to implement it, implement, discuss
// the implementation, merge. The smaller one, revealed on hover/focus, mints a
// bare worktree: somewhere to work right now, with nothing filed and nothing
// dispatched (worktree.create), for when the answer is "let me just look".
//
// Ranked rather than side by side because they are not peers: filing an issue
// is the flow, a scratch worktree is the escape hatch.

import { esc } from "./text.js";

/** Pure markup. The primary FAB comes FIRST in DOM order — that is both tab
 *  order (the main verb leads) and, because the stack is column-reverse, the
 *  bottom of the stack, leaving the mini to rise above it on hover. */
export function fabHtml({ issueLabel = "New issue", worktreeLabel = "New worktree" } = {}) {
  return `<div class="fab-stack">
    <button class="fab" type="button" data-fab="issue" title="${esc(issueLabel)}">+ ${esc(issueLabel)}</button>
    <button class="fab-mini" type="button" data-fab="worktree" title="${esc(worktreeLabel)}" aria-label="${esc(worktreeLabel)}">
      <span class="fab-mini-glyph">⑂</span><span class="fab-mini-label">${esc(worktreeLabel)}</span>
    </button>
  </div>`;
}

/**
 * mountFab(host, { onNewIssue, onNewWorktree }) → { dispose() }
 *
 * Renders the stack into `host` and wires both buttons. `onNewWorktree` is
 * awaited: while it runs the mini FAB is disabled and says so, because minting
 * a worktree is a real filesystem action and a second click would cut a second
 * branch. A rejection re-enables it (the caller has surfaced the error).
 */
export function mountFab(host, { onNewIssue, onNewWorktree } = {}) {
  host.innerHTML = fabHtml();
  const issue = host.querySelector('[data-fab="issue"]');
  const worktree = host.querySelector('[data-fab="worktree"]');
  if (issue && onNewIssue) issue.onclick = () => onNewIssue();
  if (worktree && onNewWorktree) {
    worktree.onclick = async () => {
      if (worktree.disabled) return;
      worktree.disabled = true;
      host.classList.add("fab-busy");
      try {
        await onNewWorktree();
      } catch {
        // The caller owns the error surface; just make the button usable again.
      } finally {
        worktree.disabled = false;
        host.classList.remove("fab-busy");
      }
    };
  }
  return {
    dispose() {
      host.innerHTML = "";
    },
  };
}
