import { el } from "../dom.js";
import { EXPANDED_ATTRIBUTE, patchElement } from "./domPatch.js";
import { hide, motionHooks, motionSettled, reveal, settleHidden } from "./motion.js";
import { EXITING_ATTRIBUTE, patchList } from "./patchList.js";
import { runningClock } from "./agentRailModel.js";
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
  CLIP_SELECTOR,
  COMPLETED_FOLD_HEAD_SELECTOR,
  COMPLETED_FOLD_SELECTOR,
  PILL_COUNT_SELECTOR,
  SURFACE_OVERLAY_BODY_SELECTOR,
  SURFACE_SELECTOR,
  TICKING_CLOCK_SELECTOR,
  WORKFLOW_HEAD_SELECTOR,
  agentRowHtml,
  checklistItemHtml,
  completedFoldHeadHtml,
  completedFoldHtml,
  kindViewerHtml,
  phaseSectionHtml,
  runningAndCompletedViewerHtml,
  shellRowHtml,
  surfaceOverlayHtml,
  surfacePillHtml,
  workflowChoiceHtml,
  workflowHeadHtml,
  workflowViewerHtml,
} from "./agentSurfacesRender.js";

const ROW_CLOCK_TICK_MS = 1000;

const PILL_MOTION = motionHooks({ axis: "width" });
const VIEWER_ROW_MOTION = motionHooks({ axis: "height" });

const nothingToRender = () => "";

/// Every row renderer takes the viewer's own options — how much width it has
/// and what a model id is called — so one shape serves all four kinds.
const rowsOf = (renderRow) => (rowOptions) => (row) => renderRow(row, rowOptions);

const oneListOfKind = (kind, renderOf) => ({
  frameHtmlWithEmptyLists: () => kindViewerHtml(kind, [], nothingToRender),
  lists: ({ surfaces, rowOptions }) => [
    { selector: SURFACE_SELECTOR[kind], rows: surfaceRows(kind, surfaces), render: renderOf(rowOptions) },
  ],
});

const runningAboveWhatFinished = (kind, renderOf) => ({
  frameHtmlWithEmptyLists: () => runningAndCompletedViewerHtml(kind, { running: [], completed: [] }, nothingToRender),
  lists: ({ surfaces, rowOptions }) => {
    const render = renderOf(rowOptions);
    const { running, completed } = runningAndCompletedRows(surfaceRows(kind, surfaces));
    return [
      { selector: SURFACE_SELECTOR.running, rows: running, render },
      { selector: SURFACE_SELECTOR.completed, rows: completed, render, folded: true },
    ];
  },
});

/// The running phase stands open the moment its section is made, and never
/// again: which folds are open is the reader's from there.
const openTheRunningPhase = (section, phase) => {
  if (phase.open) section.open = true;
};

const VIEWER_PLANS = {
  [WORKFLOW_ENTRY_KIND]: {
    frameHtmlWithEmptyLists: () => workflowViewerHtml({}, []),
    headSelector: WORKFLOW_HEAD_SELECTOR,
    headHtml: ({ workflow }) => workflowHeadHtml(workflow || {}),
    lists: ({ surfaces, selectedWorkflowIndex, nowMs, rowOptions }) => [
      {
        selector: SURFACE_SELECTOR.workflowChoices,
        rows: workflowChoicesWorthOffering(surfaces, selectedWorkflowIndex),
        render: workflowChoiceHtml,
      },
      {
        selector: SURFACE_SELECTOR.workflowPhases,
        rows: workflowPhases(surfaces, selectedWorkflowIndex, nowMs),
        render: (phase) => phaseSectionHtml(phase, rowOptions),
        wire: openTheRunningPhase,
        nested: (phase) => ({
          selector: SURFACE_SELECTOR.workflowAgents,
          rows: phase.rows,
          render: rowsOf(agentRowHtml)(rowOptions),
        }),
      },
    ],
  },
  [AGENT_ENTRY_KIND]: runningAboveWhatFinished(AGENT_ENTRY_KIND, rowsOf(agentRowHtml)),
  [SHELL_ENTRY_KIND]: runningAboveWhatFinished(SHELL_ENTRY_KIND, rowsOf(shellRowHtml)),
  [CHECKLIST_ENTRY_KIND]: oneListOfKind(CHECKLIST_ENTRY_KIND, rowsOf(checklistItemHtml)),
};

/// A press on clipped text opens it out, and the next one clips it back. The
/// mark is the reader's: the patch leaves it alone, and no render writes it.
function expandClippedText(event) {
  const clipped = event.target.closest(CLIP_SELECTOR);
  if (!clipped) return false;
  event.preventDefault();
  clipped.toggleAttribute(EXPANDED_ATTRIBUTE);
  return true;
}

export function mountSurfaceViewer(host, kind, { onOpenThreadItem, compact = false, modelLabel }) {
  const plan = VIEWER_PLANS[kind];
  if (!plan) throw new Error(`agentSurfaces: no viewer for kind "${kind}"`);

  const rowOptions = { compact, modelLabel };
  let surfaces = null;
  let selectedWorkflowIndex = 0;
  let ticker = null;

  const paintRowClocks = () => {
    const nowMs = Date.now();
    const spans = [...host.querySelectorAll(TICKING_CLOCK_SELECTOR)].filter(
      (span) => !span.closest(`[${EXITING_ATTRIBUTE}]`),
    );
    for (const span of spans) {
      span.textContent = runningClock((nowMs - Number(span.dataset.runningSince)) / 1000);
    }
    return spans.length;
  };

  const stopTicking = () => {
    if (ticker === null) return;
    clearInterval(ticker);
    ticker = null;
  };

  const tickWhileAnyRowIsRunning = () => {
    if (!paintRowClocks()) return stopTicking();
    if (ticker === null) ticker = setInterval(paintRowClocks, ROW_CLOCK_TICK_MS);
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

  const paintList = (container, list) => {
    const painted = patchList(container, list.rows, {
      keyOf: (row) => row.key,
      render: list.render,
      wire: list.wire,
      ...VIEWER_ROW_MOTION,
    });
    if (!list.nested) return;
    painted.forEach((element, index) => {
      const inner = list.nested(list.rows[index]);
      paintList(element.querySelector(inner.selector), inner);
    });
  };

  const paint = () => {
    const paintContext = {
      surfaces,
      workflow: openWorkflow(surfaces, selectedWorkflowIndex),
      selectedWorkflowIndex,
      nowMs: Date.now(),
      rowOptions,
    };
    if (plan.headSelector) {
      patchElement(host.querySelector(plan.headSelector), el(plan.headHtml(paintContext)));
    }
    for (const list of plan.lists(paintContext)) {
      const container = list.folded ? completedFoldContainer(list.rows.length) : host.querySelector(list.selector);
      if (!container) continue;
      paintList(container, list);
    }
    tickWhileAnyRowIsRunning();
  };

  const onViewerPress = (event) => {
    if (expandClippedText(event)) return;
    const workflow = event.target.closest("[data-workflow-index]");
    if (workflow) {
      selectedWorkflowIndex = Number(workflow.dataset.workflowIndex);
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
      stopTicking();
      host.removeEventListener("click", onViewerPress);
      host.innerHTML = "";
    },
  };
}

export function openSurfaceOverlay(kind, { onOpenThreadItem, modelLabel, onClose = null, host = document.body }) {
  let viewer = null;
  const { body, close } = openModal({
    dialogHtml: surfaceOverlayHtml(surfaceKindLabel(kind)),
    host,
    onClose: () => {
      viewer.dispose();
      if (onClose) onClose();
    },
  });
  viewer = mountSurfaceViewer(body.querySelector(SURFACE_OVERLAY_BODY_SELECTOR), kind, { onOpenThreadItem, modelLabel });
  return {
    kind,
    set(surfaces) {
      viewer.set(surfaces);
    },
    close,
  };
}

export function mountAgentSurfaces({ pillHost, viewerHost, key, onOpenThreadItem, modelLabel, onPillsChanged }) {
  let surfaces = null;
  let paintedSurfaces = null;
  let chosenKind = readOpenSurface(key);
  let viewer = null;
  let closingFrame = null;
  let visibility = emptySurfaceVisibility();
  let hidingTimer = null;

  const openKind = () => visibility.openKind;

  const closeViewerFrame = () => {
    if (!viewer) return;
    const frame = { viewer };
    closingFrame = frame;
    hide(viewerHost, { axis: "height" }).then(() => {
      if (closingFrame !== frame) return;
      closingFrame = null;
      viewer = null;
      frame.viewer.dispose();
    });
  };

  const keepTheClosingFrame = () => {
    if (!closingFrame) return;
    closingFrame = null;
    reveal(viewerHost, { axis: "height" });
  };

  const paintViewer = () => {
    const kind = openKind();
    if (viewer && viewer.kind === kind) {
      keepTheClosingFrame();
      viewer.set(surfaces);
      return;
    }
    if (!kind) {
      closeViewerFrame();
      return;
    }
    closingFrame = null;
    if (viewer) viewer.dispose();
    viewer = mountSurfaceViewer(viewerHost, kind, { onOpenThreadItem, modelLabel, compact: true });
    reveal(viewerHost, { axis: "height" });
    viewer.set(surfaces);
  };

  const paintPillCount = (pillElement, count) => {
    const cap = pillElement.querySelector(PILL_COUNT_SELECTOR);
    if (count) reveal(cap, { axis: "width" });
    else hide(cap, { axis: "width" });
  };

  const notifyPillsChanged = () => {
    if (onPillsChanged) onPillsChanged();
  };

  const paintPills = (nowMs) => {
    const pills = surfacePills(surfaces, visibility, nowMs);
    const painted = patchList(pillHost, pills, {
      keyOf: (pill) => pill.kind,
      render: (pill) => surfacePillHtml(pill, openKind()),
      ...PILL_MOTION,
    });
    painted.forEach((element, index) => paintPillCount(element, pills[index].count));
    notifyPillsChanged();
    motionSettled().then(notifyPillsChanged);
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

  pillHost.addEventListener("click", onPillPress);

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
      closingFrame = null;
      if (viewer) viewer.dispose();
      viewer = null;
      settleHidden(viewerHost);
      pillHost.removeEventListener("click", onPillPress);
      paintedSurfaces = null;
      pillHost.innerHTML = "";
      notifyPillsChanged();
    },
  };
}
