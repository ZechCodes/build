// A workspace's Changes: a tab row naming every directory the workspace has,
// over the git surface of the one that is selected (#174).
//
// "For git I want to add a tab row above the git surface for each directory in
// the workspace (git or not). This should be to the right of the left rail
// since it's lower in the hierarchy. Just make sure that non-git directories
// are shown with the option to init git."
//
// The row is a pane element — not the shell's toolbar, not the rail — so where
// the list column is a drawer (a phone) it still stands above the surface. A
// directory with git shows the commit rail and the detail (core/gitPane.js)
// under its ref picker; one without shows the offer to initialize it as the
// surface's content, hung there by the view (views/workspaceView.js), and
// becomes the commit rail and the detail when its record says it has git,
// without the view being built again.

import { esc } from "../core/text.js";
import { mountGitPane } from "../core/gitPane.js";
import { mountWorkspaceRefPicker } from "../core/workspaceRefPicker.js";

/** Where an arrow takes the selection along the row. */
const KEY_STEPS = { ArrowLeft: -1, ArrowRight: 1 };
const KEY_ENDS = { Home: 0, End: -1 };

/** Whether a directory has git to show: only a record saying it has none is
 *  one without. */
export const directoryHasGit = (directory) => directory?.is_git !== false;

/** Pure: the tab row — one tab per directory, in the workspace's order, git or
 *  not, the current one selected and the one Tab lands on. Names come from the
 *  user and the repos, so they are escaped. */
export function directoryTabsHtml(directories) {
  const tabs = directories
    .map(
      (directory) =>
        `<button class="workspace-dirtab${directory.current ? " current" : ""}" type="button" role="tab" data-directory="${esc(directory.sourceId)}" aria-selected="${directory.current}" tabindex="${directory.current ? 0 : -1}">${esc(directory.label)}</button>`,
    )
    .join("");
  return `<div class="workspace-dirtabs" role="tablist" aria-label="Workspace directories">${tabs}</div>`;
}

/** A selection made from the keyboard navigates, and the next Changes to mount
 *  hands the keyboard back to the row — the arrows keep walking it. */
let keyboardOnRow = false;

function wireDirectoryTabs(row, onSelect) {
  const cells = () => [...row.querySelectorAll("[data-directory]")];
  row.onclick = (event) => {
    const cell = event.target.closest("[data-directory]");
    if (cell) onSelect(cell.dataset.directory);
  };
  row.onkeydown = (event) => {
    const step = KEY_STEPS[event.key];
    const end = KEY_ENDS[event.key];
    const all = cells();
    const from = all.indexOf(event.target.closest("[data-directory]"));
    if ((step === undefined && end === undefined) || from < 0) return;
    event.preventDefault();
    const to = step === undefined ? all.at(end) : all[(from + step + all.length) % all.length];
    to.focus();
    keyboardOnRow = true;
    onSelect(to.dataset.directory);
  };
  if (keyboardOnRow) row.querySelector("[aria-selected='true']")?.focus();
  keyboardOnRow = false;
}

/** The git surface of one directory: its ref picker at the head of the commit
 *  rail, and the pane mounted again whenever a checkout moves the ref. */
export function mountChanges(body, { scope, callRpc, cacheScope, projectId, navigate, viewingContext, agentSelection, requestedCommit, onCommitSelection }) {
  body.innerHTML = `<div class="workspace-gitpane"></div>`;
  const refbar = document.createElement("div");
  refbar.className = "workspace-refbar";
  const gitHost = body.querySelector(".workspace-gitpane");
  let gitPane = mountGitPane(gitHost, { scope, callRpc, cacheScope, projectId, navigate, viewingContext, agentSelection,
    requestedCommit, onCommitSelection });
  let disposed = false;
  const attachRefbar = () => {
    const rail = gitHost.querySelector(".crail-host");
    if (!rail) return false;
    if (refbar.parentElement === rail && rail.firstElementChild === refbar) return true;
    rail.prepend(refbar);
    return true;
  };
  const attachObserver = new MutationObserver(attachRefbar);
  attachRefbar();
  attachObserver.observe(gitHost, { childList: true, subtree: true });
  const refPicker = mountWorkspaceRefPicker(refbar, { scope, callRpc, cacheScope, onCheckout: async () => {
      if (disposed) return;
      gitPane.dispose();
      onCommitSelection?.(null);
      gitPane = mountGitPane(gitHost, { scope, callRpc, cacheScope, projectId, navigate, viewingContext, agentSelection,
        requestedCommit: null, onCommitSelection });
    } });
  return { dispose: () => {
    disposed = true;
    attachObserver.disconnect();
    refPicker.dispose();
    gitPane.dispose();
  } };
}

/** What a directory with no git shows in place of the commit rail: the offer to
 *  initialize it, hung in `.workspace-gitinit-offer` by the view. */
const noGitHtml = (directory) =>
  `<div class="workspace-gitinit"><p class="workspace-gitinit-say">${esc(directory.label)} has no Git repository.</p><div class="workspace-gitinit-offer"></div></div>`;

/**
 * mountWorkspaceChanges(body, { directories, git, onSelectDirectory }) — the
 * row and the surface under it. `directories` are the workspace's
 * (core/workspaceModel.js workspaceDirectoryModel), the route's one current;
 * `git` is the mountChanges options for it. Returns { dispose,
 * workspaceMoved(directories) }: a move repaints the row, and turns the offer
 * into the git surface once the current directory's record says it has git.
 */
export function mountWorkspaceChanges(body, { directories, git, onSelectDirectory }) {
  body.innerHTML = `<div class="workspace-changes">${directoryTabsHtml(directories)}<div class="workspace-changes-body"></div></div>`;
  const surface = body.querySelector(".workspace-changes-body");
  const current = () => directories.find((directory) => directory.current) || directories[0];
  let pane = null;
  const paintSurface = () => {
    if (directoryHasGit(current())) pane = mountChanges(surface, git);
    else surface.innerHTML = noGitHtml(current());
  };
  let rowHtml = directoryTabsHtml(directories);
  // Only a row that says something new is drawn again: the keyboard can be
  // standing on it.
  const paintRow = () => {
    const next = directoryTabsHtml(directories);
    if (next === rowHtml) return;
    rowHtml = next;
    body.querySelector(".workspace-dirtabs").outerHTML = next;
    wireDirectoryTabs(body.querySelector(".workspace-dirtabs"), onSelectDirectory);
  };
  wireDirectoryTabs(body.querySelector(".workspace-dirtabs"), onSelectDirectory);
  paintSurface();
  return {
    workspaceMoved(next) {
      const hadGit = directoryHasGit(current());
      directories = next;
      paintRow();
      if (!hadGit && directoryHasGit(current())) paintSurface();
    },
    dispose: () => pane?.dispose(),
  };
}
