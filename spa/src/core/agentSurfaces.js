import { el } from "../dom.js";
import { patchElement } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { SPLIT_BUTTON_SELECTOR, mountSplitMenu } from "./splitButton.js";
import { notifyError } from "./notify.js";
import { openModal } from "./modal.js";
import {
  AGENT_ENTRY_KIND,
  CHECKLIST_ENTRY_KIND,
  SHELL_ENTRY_KIND,
  WORKFLOW_ENTRY_KIND,
  advanceSurfaceVisibility,
  emptySurfaceVisibility,
  nextSurfacePillExpiry,
  openSurfaceKind,
  openWorkflow,
  openedSurfaceVisibility,
  readOpenSurface,
  runningAndCompletedRows,
  surfaceKindLabel,
  surfacePills,
  surfaceRows,
  workflowChoicesWorthOffering,
  workflowPhases,
  writeOpenSurface,
} from "./agentSurfacesModel.js";
import {
  COMPLETED_FOLD_HEAD_SELECTOR,
  COMPLETED_FOLD_SELECTOR,
  SURFACE_OVERLAY_BODY_SELECTOR,
  SURFACE_SELECTOR,
  WORKFLOW_HEAD_SELECTOR,
  agentRowHtml,
  checklistItemHtml,
  completedFoldHeadHtml,
  completedFoldHtml,
  kindViewerHtml,
  runningAndCompletedViewerHtml,
  shellRowHtml,
  surfaceOverlayHtml,
  surfacePillsHtml,
  workflowChoiceHtml,
  workflowHeadHtml,
  workflowPhaseHtml,
  workflowViewerHtml,
} from "./agentSurfacesRender.js";

const VIEWER_ABOVE_PILLS_HTML = `<div class="rail-surfaces-viewer" data-surface-viewer></div>
  <div class="rail-surfaces-pills" data-surface-pills></div>`;

const oneListOfKind = (kind, render) => ({
  frameHtmlWithEmptyLists: () => kindViewerHtml(kind, [], render),
  lists: ({ surfaces }) => [
    { selector: SURFACE_SELECTOR[kind], rows: surfaceRows(kind, surfaces), render, carriesRowActions: true },
  ],
});

const runningAboveWhatFinished = (kind, render) => ({
  frameHtmlWithEmptyLists: () => runningAndCompletedViewerHtml(kind, { running: [], completed: [] }, render),
  lists: ({ surfaces }) => {
    const { running, completed } = runningAndCompletedRows(surfaceRows(kind, surfaces));
    return [
      { selector: SURFACE_SELECTOR.running, rows: running, render, carriesRowActions: true },
      { selector: SURFACE_SELECTOR.completed, rows: completed, render, carriesRowActions: true, folded: true },
    ];
  },
});

const VIEWER_PLANS = {
  [WORKFLOW_ENTRY_KIND]: {
    frameHtmlWithEmptyLists: () => workflowViewerHtml({}, [], [], []),
    headSelector: WORKFLOW_HEAD_SELECTOR,
    headHtml: ({ workflow }) => workflowHeadHtml(workflow || {}),
    lists: ({ surfaces, selectedWorkflowIndex, selectedPhaseIndex }) => {
      const { phases, agents } = workflowPhases(surfaces, selectedWorkflowIndex, selectedPhaseIndex);
      return [
        {
          selector: SURFACE_SELECTOR.workflowChoices,
          rows: workflowChoicesWorthOffering(surfaces, selectedWorkflowIndex),
          render: workflowChoiceHtml,
        },
        { selector: SURFACE_SELECTOR.workflowPhases, rows: phases, render: workflowPhaseHtml },
        {
          selector: SURFACE_SELECTOR.workflowAgents,
          rows: agents,
          render: agentRowHtml,
          carriesRowActions: true,
        },
      ];
    },
  },
  [AGENT_ENTRY_KIND]: runningAboveWhatFinished(AGENT_ENTRY_KIND, agentRowHtml),
  [SHELL_ENTRY_KIND]: runningAboveWhatFinished(SHELL_ENTRY_KIND, shellRowHtml),
  [CHECKLIST_ENTRY_KIND]: oneListOfKind(CHECKLIST_ENTRY_KIND, checklistItemHtml),
};

export function mountSurfaceViewer(host, kind, { onSendMessage, onOpenThreadItem }) {
  const plan = VIEWER_PLANS[kind];
  if (!plan) throw new Error(`agentSurfaces: no viewer for kind "${kind}"`);

  let surfaces = null;
  let selectedWorkflowIndex = 0;
  let selectedPhaseIndex = 0;
  const paintedLists = new Map();
  const menuClosersByElement = new Map();

  const sendRowMessage = async (message) => {
    try {
      await onSendMessage(message);
    } catch (error) {
      notifyError("Could not ask the agent", error.message);
    }
  };

  const chooseRowAction = (row, actionId) => {
    const action = (row.actions || []).find((candidate) => candidate.id === actionId);
    if (!action) throw new Error(`agentSurfaces: no action "${actionId}" on row "${row.key}"`);
    sendRowMessage(action.message);
  };

  const rowUnderMenu = (rowElement, selector) => {
    const painted = paintedLists.get(selector);
    return painted ? painted.rows.find((candidate) => candidate.key === rowElement.dataset.key) : null;
  };

  const chooseTheHeadsAction = (actionId) => {
    const workflow = openWorkflow(surfaces, selectedWorkflowIndex);
    if (!workflow) {
      notifyError("Could not ask the agent", "that workflow is gone");
      return;
    }
    chooseRowAction(workflow, actionId);
  };

  const chooseTheRowsAction = (rowElement, selector, actionId) => {
    const row = rowUnderMenu(rowElement, selector);
    if (!row) {
      notifyError("Could not ask the agent", "that row is gone");
      return;
    }
    chooseRowAction(row, actionId);
  };

  const actionChooserFor = (menuElement) => {
    if (plan.headSelector && menuElement.closest(plan.headSelector)) return chooseTheHeadsAction;
    for (const [selector, list] of paintedLists) {
      if (!list.carriesRowActions) continue;
      const container = host.querySelector(selector);
      if (!container || !container.contains(menuElement)) continue;
      const rowElement = menuElement.closest("[data-key]");
      return (actionId) => chooseTheRowsAction(rowElement, selector, actionId);
    }
    return null;
  };

  const wireUnwiredMenus = () => {
    for (const menuElement of host.querySelectorAll(SPLIT_BUTTON_SELECTOR)) {
      if (menuClosersByElement.has(menuElement)) continue;
      const chooseAction = actionChooserFor(menuElement);
      if (!chooseAction) continue;
      const { closeMenu } = mountSplitMenu(menuElement, { onChoose: chooseAction });
      menuClosersByElement.set(menuElement, closeMenu);
    }
  };

  const closeMenusOfDiscardedElements = () => {
    for (const [element, closeMenu] of [...menuClosersByElement]) {
      if (host.contains(element)) continue;
      closeMenu();
      menuClosersByElement.delete(element);
    }
  };

  const completedFoldContainer = (count) => {
    const standing = host.querySelector(COMPLETED_FOLD_SELECTOR);
    if (!count) {
      if (standing) standing.remove();
      return null;
    }
    if (!standing) {
      host.querySelector(SURFACE_SELECTOR.viewer).appendChild(el(completedFoldHtml(count)));
      return host.querySelector(SURFACE_SELECTOR.completed);
    }
    patchElement(standing.querySelector(COMPLETED_FOLD_HEAD_SELECTOR), el(completedFoldHeadHtml(count)));
    return standing.querySelector(SURFACE_SELECTOR.completed);
  };

  const paint = () => {
    paintedLists.clear();
    const paintContext = {
      surfaces,
      workflow: openWorkflow(surfaces, selectedWorkflowIndex),
      selectedWorkflowIndex,
      selectedPhaseIndex,
    };
    if (plan.headSelector) {
      patchElement(host.querySelector(plan.headSelector), el(plan.headHtml(paintContext)));
    }
    for (const list of plan.lists(paintContext)) {
      const container = list.folded ? completedFoldContainer(list.rows.length) : host.querySelector(list.selector);
      if (!container) continue;
      paintedLists.set(list.selector, list);
      patchList(container, list.rows, {
        keyOf: (row) => row.key,
        render: list.render,
      });
    }
    closeMenusOfDiscardedElements();
    wireUnwiredMenus();
  };

  const onViewerPress = (event) => {
    if (event.target.closest(SPLIT_BUTTON_SELECTOR)) return;
    const workflow = event.target.closest("[data-workflow-index]");
    if (workflow) {
      selectedWorkflowIndex = Number(workflow.dataset.workflowIndex);
      selectedPhaseIndex = 0;
      paint();
      return;
    }
    const phase = event.target.closest("[data-phase-index]");
    if (phase) {
      selectedPhaseIndex = Number(phase.dataset.phaseIndex);
      paint();
      return;
    }
    const spawned = event.target.closest("[data-call-sequence]");
    if (spawned && onOpenThreadItem) onOpenThreadItem(Number(spawned.dataset.callSequence));
  };

  host.innerHTML = plan.frameHtmlWithEmptyLists();
  host.addEventListener("click", onViewerPress);

  return {
    kind,
    set(nextSurfaces) {
      surfaces = nextSurfaces || null;
      paint();
    },
    dispose() {
      for (const closeMenu of menuClosersByElement.values()) closeMenu();
      menuClosersByElement.clear();
      host.removeEventListener("click", onViewerPress);
      paintedLists.clear();
      host.innerHTML = "";
    },
  };
}

export function openSurfaceOverlay(kind, { onSendMessage, onOpenThreadItem, onClose = null, host = document.body }) {
  let viewer = null;
  const { body, close } = openModal({
    dialogHtml: surfaceOverlayHtml(surfaceKindLabel(kind)),
    host,
    onClose: () => {
      viewer.dispose();
      if (onClose) onClose();
    },
  });
  viewer = mountSurfaceViewer(body.querySelector(SURFACE_OVERLAY_BODY_SELECTOR), kind, {
    onSendMessage,
    onOpenThreadItem,
  });
  return {
    kind,
    set(surfaces) {
      viewer.set(surfaces);
    },
    close,
  };
}

export function mountAgentSurfaces(host, { key, onSendMessage, onOpenThreadItem }) {
  host.innerHTML = VIEWER_ABOVE_PILLS_HTML;
  const viewerRegion = host.querySelector("[data-surface-viewer]");
  const pillRegion = host.querySelector("[data-surface-pills]");

  let surfaces = null;
  let paintedSurfaces = null;
  let chosenKind = readOpenSurface(key);
  let viewer = null;
  let visibility = emptySurfaceVisibility();
  let hidingTimer = null;

  const openKind = () => visibility.openKind;

  const paintViewer = () => {
    if (viewer && viewer.kind !== openKind()) {
      viewer.dispose();
      viewer = null;
    }
    if (!openKind()) return;
    if (!viewer) viewer = mountSurfaceViewer(viewerRegion, openKind(), { onSendMessage, onOpenThreadItem });
    viewer.set(surfaces);
  };

  const paintPills = (nowMs) => {
    const html = surfacePillsHtml(surfacePills(surfaces, visibility, nowMs), openKind());
    const next = html ? el(html) : null;
    const live = pillRegion.firstElementChild;
    if (live && next && live.tagName === next.tagName) {
      patchElement(live, next);
      return;
    }
    pillRegion.innerHTML = html;
  };

  const armPillHidingTimer = (nowMs) => {
    if (hidingTimer !== null) clearTimeout(hidingTimer);
    hidingTimer = null;
    const expiry = nextSurfacePillExpiry(surfaces, visibility, nowMs);
    if (expiry === null) return;
    hidingTimer = setTimeout(() => {
      hidingTimer = null;
      paint();
    }, expiry - nowMs);
  };

  const paint = () => {
    const nowMs = Date.now();
    visibility = openedSurfaceVisibility(visibility, openSurfaceKind(surfaces, chosenKind, visibility, nowMs), nowMs);
    paintViewer();
    paintPills(nowMs);
    armPillHidingTimer(nowMs);
  };

  const onPillPress = (event) => {
    const button = event.target.closest("[data-surface-kind]");
    if (!button) return;
    const kind = button.dataset.surfaceKind;
    chosenKind = kind === openKind() ? null : kind;
    writeOpenSurface(key, chosenKind);
    paint();
  };

  pillRegion.addEventListener("click", onPillPress);

  return {
    set(nextSurfaces) {
      const arriving = JSON.stringify(nextSurfaces || null);
      const unchangedSinceLastPaint = arriving === paintedSurfaces;
      paintedSurfaces = arriving;
      surfaces = nextSurfaces || null;
      visibility = advanceSurfaceVisibility(visibility, surfaces, Date.now());
      if (unchangedSinceLastPaint) return;
      paint();
    },
    dispose() {
      if (hidingTimer !== null) clearTimeout(hidingTimer);
      hidingTimer = null;
      if (viewer) viewer.dispose();
      viewer = null;
      pillRegion.removeEventListener("click", onPillPress);
      paintedSurfaces = null;
      host.innerHTML = "";
    },
  };
}
