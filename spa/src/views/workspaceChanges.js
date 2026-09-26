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
//
// Selecting a tab moves within the surface, not to another one: each
// directory's surface is mounted the first time it is selected and kept, so
// going back to one shows it again over the records its readers kept true
// while it was hidden, and nothing it holds is asked for again.

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
    onSelect(to.dataset.directory);
  };
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
  let visible = true;
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
      gitPane.setVisible?.(visible);
    } });
  return { setVisible(shown) {
    visible = shown;
    refPicker.setVisible?.(shown);
    gitPane.setVisible?.(shown);
  }, dispose: () => {
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

/** What the reader is looking at, as a kept surface may say it: only the shown
 *  one's word reaches the page. A hidden surface still repaints on a push, and
 *  what it would say then is kept, and said when it is shown again. */
const CONTEXT_ARTIFACT = new Set(["set", "clear", "setVisibleDiffs"]);
const CONTEXT_SELECTION = new Set(["setSelection", "captureDomSelection", "clearSelection", "clearSelectionIfMatches"]);

function gatedViewingContext(viewingContext) {
  if (!viewingContext) return { context: null, show() {}, hide() {} };
  let shown = true;
  let last = null;
  let snapshot = null;
  const context = new Proxy(viewingContext, {
    get(target, name) {
      const value = target[name];
      if (typeof value !== "function") return value;
      if (name === "snapshot") return (...args) => shown ? value.apply(target, args) : snapshot;
      if (CONTEXT_ARTIFACT.has(name)) return (...args) => {
        last = () => target[name](...args);
        if (shown) last();
      };
      if (CONTEXT_SELECTION.has(name)) return (...args) => (shown ? target[name](...args) : undefined);
      return value.bind(target);
    },
  });
  return {
    context,
    show() {
      shown = true;
      viewingContext.clear();
      last?.();
    },
    hide() {
      snapshot = viewingContext.snapshot?.() || null;
      shown = false;
    },
  };
}

/**
 * mountWorkspaceChanges(body, { directories, current, git, viewingContext,
 * onSelectDirectory }) — the row and the surface under it. `directories` are
 * the workspace's (core/workspaceModel.js workspaceDirectoryModel), `current`
 * the source id standing; `git(sourceId, viewingContext)` is the mountChanges
 * options for a directory. A tab calls `onSelectDirectory(sourceId)` once its
 * surface is showing. Returns { dispose, workspaceMoved(directories) }: a move
 * repaints the row, and turns a directory's offer into the git surface once its
 * record says it has git.
 */
export function mountWorkspaceChanges(body, { directories, current, git, viewingContext = null, onSelectDirectory }) {
  let standing = current;
  const model = () => directories.map((directory) => ({ ...directory, current: directory.sourceId === standing }));
  body.innerHTML = `<div class="workspace-changes">${directoryTabsHtml(model())}<div class="workspace-changes-body"></div></div>`;
  const host = body.querySelector(".workspace-changes-body");
  const surfaces = new Map(); // source id → { element, pane, gate }
  const directoryOf = (sourceId) => directories.find((directory) => directory.sourceId === sourceId);

  const paintSurface = (surface, sourceId) => {
    const directory = directoryOf(sourceId);
    if (!directory) return;
    if (directoryHasGit(directory)) {
      surface.pane = mountChanges(surface.element, git(sourceId, surface.gate.context));
      surface.pane.setVisible(!surface.element.hidden);
    }
    else surface.element.innerHTML = noGitHtml(directory);
  };
  const surfaceFor = (sourceId) => {
    if (surfaces.has(sourceId)) return surfaces.get(sourceId);
    const element = document.createElement("div");
    element.className = "workspace-changes-surface";
    element.dataset.surface = sourceId;
    host.appendChild(element);
    const surface = { element, pane: null, gate: gatedViewingContext(viewingContext) };
    surfaces.set(sourceId, surface);
    paintSurface(surface, sourceId);
    return surface;
  };

  let rowHtml = directoryTabsHtml(model());
  // Only a row that says something new is drawn again: the keyboard can be
  // standing on it.
  const paintRow = () => {
    const next = directoryTabsHtml(model());
    if (next === rowHtml) return;
    rowHtml = next;
    // The keyboard on the row stays on it, on the tab now selected: the arrows
    // keep walking it.
    const focused = body.querySelector(".workspace-dirtabs").contains(document.activeElement);
    body.querySelector(".workspace-dirtabs").outerHTML = next;
    const row = body.querySelector(".workspace-dirtabs");
    wireDirectoryTabs(row, select);
    if (focused) row.querySelector("[aria-selected='true']")?.focus();
  };
  // Visibility owns interactions; lifetime owns cache subscriptions and drafts.
  // Suspend before hiding so each child can release focus and measure its place.
  function select(sourceId) {
    if (sourceId === standing || !directoryOf(sourceId)) return;
    const leaving = surfaces.get(standing);
    if (leaving) {
      leaving.gate.hide();
      leaving.pane?.setVisible(false);
      if (leaving.element.contains(document.activeElement)) document.activeElement.blur();
    }
    standing = sourceId;
    const shown = surfaceFor(sourceId);
    for (const surface of surfaces.values()) {
      surface.element.hidden = surface !== shown;
      surface.element.inert = surface !== shown;
    }
    paintRow();
    onSelectDirectory(sourceId);
    shown.gate.show();
    shown.pane?.setVisible(true);
  }
  wireDirectoryTabs(body.querySelector(".workspace-dirtabs"), select);
  surfaceFor(standing);
  return {
    workspaceMoved(next) {
      const hadGit = new Map([...surfaces.keys()].map((sourceId) => [sourceId, directoryHasGit(directoryOf(sourceId))]));
      directories = next;
      paintRow();
      for (const [sourceId, surface] of surfaces) {
        if (!hadGit.get(sourceId) && directoryHasGit(directoryOf(sourceId))) paintSurface(surface, sourceId);
      }
    },
    dispose: () => surfaces.forEach((surface) => surface.pane?.dispose()),
  };
}
