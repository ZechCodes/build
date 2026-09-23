import { el } from "../dom.js";
import { readerIsMoving, writeScrollTop } from "./paintKeepingPlace.js";
import { EXPANDED_ATTRIBUTE, patchElement } from "./domPatch.js";
import { hide, motionHooks, motionSettled, reveal, settleHidden } from "./motion.js";
import { EXITING_ATTRIBUTE, patchList } from "./patchList.js";
import { elapsedClock } from "./agentRailModel.js";
import { checklistObservationModel } from "./agentObservationModel.js";
import { ICON_HISTORY, ICON_X } from "./icons.js";
import { openModal } from "./modal.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import {
  AGENT_ENTRY_KIND,
  CHECKLIST_ENTRY_KIND,
  ISSUES_ENTRY_KIND,
  SHELL_ENTRY_KIND,
  WORKFLOW_ENTRY_KIND,
  advanceSurfaceVisibility,
  emptySurfaceVisibility,
  nextSurfacePillExpiry,
  openSurfaceKind,
  openWorkflow,
  openedSurfaceVisibility,
  runningAndCompletedRows,
  surfaceKindLabel,
  surfacePills,
  surfaceRows,
  workflowPhases,
} from "./agentSurfacesModel.js";
import {
  PRESSABLE_CLIP_SELECTOR,
  CHECKLIST_CONTEXT_SELECTOR,
  COMPLETED_FOLD_HEAD_SELECTOR,
  COMPLETED_FOLD_SELECTOR,
  PILL_COUNT_SELECTOR,
  SURFACE_OVERLAY_BODY_SELECTOR,
  SURFACE_SELECTOR,
  TICKING_CLOCK_SELECTOR,
  WORKFLOW_HEAD_SELECTOR,
  agentRowHtml,
  checklistItemHtml,
  issueSurfaceRowHtml,
  checklistContextHtml,
  checklistViewerHtml,
  completedFoldHeadHtml,
  completedFoldHtml,
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
const SURFACE_CLEARANCE_PROPERTY = "--surface-popover-clearance";

const nothingToRender = () => "";

const PILL_MOTION = motionHooks({ axis: "width" });
const VIEWER_ROW_MOTION = motionHooks({ axis: "height" });

export function mountSurfaceClearance(viewerHost) {
  const scroller = viewerHost.closest(".rail-panel")?.querySelector("#rail-body");
  if (!scroller || typeof ResizeObserver === "undefined") return () => {};

  const sync = () => {
    const distanceFromBottom = scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop;
    const wasAtBottom = distanceFromBottom <= 2;
    const margin = Number.parseFloat(getComputedStyle(viewerHost).marginBottom) || 0;
    const height = viewerHost.hidden ? 0 : viewerHost.getBoundingClientRect().height + margin;
    scroller.style.setProperty(SURFACE_CLEARANCE_PROPERTY, `${height}px`);
    if (wasAtBottom && !readerIsMoving(scroller)) writeScrollTop(scroller, scroller.scrollHeight);
  };

  const observer = new ResizeObserver(sync);
  observer.observe(viewerHost);
  sync();
  return () => {
    observer.disconnect();
    scroller.style.removeProperty(SURFACE_CLEARANCE_PROPERTY);
  };
}

const runningAboveWhatFinished = (kind, renderRow) => ({
  frameHtmlWithEmptyLists: () => runningAndCompletedViewerHtml(kind, { running: [], completed: [] }, nothingToRender),
  lists: ({ surfaces, reading, rowOptions }) => {
    const render = (row) => renderRow(row, rowOptions);
    const { running, completed } = runningAndCompletedRows(surfaceRows(kind, surfaces, reading));
    return [
      { selector: SURFACE_SELECTOR.running, rows: running, render },
      { selector: SURFACE_SELECTOR.completed, rows: completed, render, folded: true },
    ];
  },
});

const openNewlyRunningPhase = (opened, section, phase) => {
  if (!phase.open || opened.has(phase.key)) return;
  opened.add(phase.key);
  section.open = true;
};

function workflowView(surfaces, reading, selectedKey, wasRunning) {
  const rows = surfaceRows(WORKFLOW_ENTRY_KIND, surfaces, reading);
  const groups = runningAndCompletedRows(rows);
  const workflow = rows.find((row) => row.key === selectedKey) || groups.running[0] || groups.completed[0];
  const isCompleted = groups.completed.includes(workflow);
  return {
    rows, groups, workflow, isCompleted,
    selectedIndex: rows.indexOf(workflow),
    selectedKey: workflow?.key ?? null,
    isRunning: !!workflow && !isCompleted,
    justFinished: workflow?.key === selectedKey && wasRunning && isCompleted,
  };
}

const VIEWER_PLANS = {
  [WORKFLOW_ENTRY_KIND]: {
    frameHtmlWithEmptyLists: () => workflowViewerHtml({}, []),
  },
  [AGENT_ENTRY_KIND]: runningAboveWhatFinished(AGENT_ENTRY_KIND, agentRowHtml),
  [SHELL_ENTRY_KIND]: runningAboveWhatFinished(SHELL_ENTRY_KIND, shellRowHtml),
  [ISSUES_ENTRY_KIND]: runningAboveWhatFinished(ISSUES_ENTRY_KIND, issueSurfaceRowHtml),
  [CHECKLIST_ENTRY_KIND]: {
    frameHtmlWithEmptyLists: () => checklistViewerHtml(),
    headSelector: CHECKLIST_CONTEXT_SELECTOR,
    headHtml: ({ surfaces }) => checklistContextHtml(checklistObservationModel(surfaces)),
    lists: ({ surfaces, reading }) => [{
      selector: SURFACE_SELECTOR.checklistRows,
      rows: surfaceRows(CHECKLIST_ENTRY_KIND, surfaces, reading),
      render: checklistItemHtml,
    }],
  },
};

function expandClippedText(event) {
  const clipped = event.target.closest(PRESSABLE_CLIP_SELECTOR);
  if (!clipped) return false;
  event.preventDefault();
  clipped.setAttribute("aria-expanded", String(clipped.toggleAttribute(EXPANDED_ATTRIBUTE)));
  return true;
}

const CLIP_KEYS = ["Enter", " "];

export function mountSurfaceViewer(host, kind, { onOpenThreadItem, compact = false, modelLabel, historyControl = null, cacheKey = null }) {
  const plan = VIEWER_PLANS[kind];
  if (!plan) throw new Error(`agentSurfaces: no viewer for kind "${kind}"`);

  const openedAgentKeys = new Set();
  const rowOptions = { compact, openedAgentKeys };
  const openedPhases = new Set();
  let surfaces = null;
  let selectedWorkflowKey = null;
  let selectedWorkflowWasRunning = false;
  let ticker = null;
  let completedCount = 0;
  let historyOpen = false;
  let foldState = {};
  let foldKnown = !cacheKey;
  let foldRecord = null;
  let disposed = false;
  let foldPainted = false;
  let autoHistoryPending = false;

  const foldKey = (element) => {
    if (element.matches(COMPLETED_FOLD_SELECTOR)) return "history";
    if (element.matches(".surface-agent[data-key]")) return `agent:${element.dataset.key}`;
    if (element.matches(".surface-phase[data-key]")) return `phase:${element.dataset.key}`;
    if (element.matches(".surface-shell-tail")) return `tail:${element.closest(".surface-row[data-key]")?.dataset.key || ""}`;
    return null;
  };
  const writeFold = (key, open, extra = {}) => {
    if (!foldRecord || !key || foldState[key] === open) return;
    void foldRecord.write({ ...foldState, [key]: open, ...extra });
  };
  const setHistoryOpen = (next, animate = false) => {
    const completed = host.querySelector(COMPLETED_FOLD_SELECTOR);
    historyOpen = next;
    if (!completed) return syncHistoryControl();
    if (next) {
      completed.open = true;
      if (animate) {
        completed.hidden = true;
        reveal(completed, { axis: "height" });
      }
    } else if (animate) {
      for (const row of completed.querySelectorAll("details.surface-agent[open]")) {
        openedAgentKeys.delete(row.dataset.key);
        row.open = false;
      }
      hide(completed, { axis: "height" }).then(() => {
        if (historyOpen || !completed.isConnected) return;
        completed.open = false;
        completed.hidden = false;
      });
    } else {
      completed.open = false;
    }
    syncHistoryControl();
  };
  const applyCachedFolds = () => {
    if (!foldRecord) return;
    for (const detail of host.querySelectorAll("details")) {
      const key = foldKey(detail);
      if (key && key !== "history" && Object.hasOwn(foldState, key)) detail.open = foldState[key];
    }
    setHistoryOpen(Boolean(foldState.history), foldPainted && historyOpen !== Boolean(foldState.history));
    foldPainted = true;
  };

  const syncHistoryControl = (count = completedCount) => {
    if (!historyControl) return;
    const action = historyOpen ? "Hide" : "Show";
    const label = `${action} completed history (${count})`;
    historyControl.hidden = count === 0;
    historyControl.disabled = count === 0;
    historyControl.setAttribute("aria-pressed", String(historyOpen));
    historyControl.setAttribute("aria-label", label);
    historyControl.title = label;
    const countElement = historyControl.querySelector(".surface-history-count");
    if (countElement) countElement.textContent = String(count);
  };

  const paintRowClocks = () => {
    const nowMs = Date.now();
    const spans = [...host.querySelectorAll(TICKING_CLOCK_SELECTOR)].filter(
      (span) => !span.closest(`[${EXITING_ATTRIBUTE}]`),
    );
    for (const span of spans) {
      span.textContent = elapsedClock(Number(span.dataset.runningSince), nowMs);
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
    completedCount = count;
    const standing = host.querySelector(COMPLETED_FOLD_SELECTOR);
    if (!count) {
      if (standing) standing.remove();
      historyOpen = false;
      syncHistoryControl();
      return null;
    }
    if (!standing) {
      host.querySelector(SURFACE_SELECTOR.viewer).appendChild(el(completedFoldHtml(count)));
    } else {
      patchElement(standing.querySelector(COMPLETED_FOLD_HEAD_SELECTOR), el(completedFoldHeadHtml(count)));
    }
    const completed = host.querySelector(COMPLETED_FOLD_SELECTOR);
    completed.querySelector(COMPLETED_FOLD_HEAD_SELECTOR).hidden = !!historyControl;
    syncHistoryControl();
    return completed.querySelector(SURFACE_SELECTOR.completed);
  };

  const paintList = (container, list) => {
    const painted = patchList(container, list.rows, {
      keyOf: (row) => row.key,
      render: list.render,
      ...VIEWER_ROW_MOTION,
    });
    painted.forEach((element, index) => {
      const row = list.rows[index];
      if (list.onPainted) list.onPainted(element, row);
      if (!list.nested) return;
      const inner = list.nested(row);
      paintList(element.querySelector(inner.selector), inner);
    });
    if (list.folded && list.rows.some((row) => openedAgentKeys.has(row.key))) {
      if (foldRecord) {
        if (!autoHistoryPending && foldState.history !== true) {
          autoHistoryPending = true;
          void foldRecord.write({ ...foldState, history: true }).finally(() => { autoHistoryPending = false; });
        }
      } else {
        historyOpen = true;
        container.closest(COMPLETED_FOLD_SELECTOR).open = true;
      }
    }
  };

  const paintWorkflow = (reading) => {
    const view = workflowView(surfaces, reading, selectedWorkflowKey, selectedWorkflowWasRunning);
    const { rows, groups, workflow, isCompleted, selectedIndex, justFinished } = view;
    selectedWorkflowKey = view.selectedKey;
    selectedWorkflowWasRunning = view.isRunning;

    const detail = host.querySelector(SURFACE_SELECTOR.workflowDetail);
    const viewer = host.querySelector(SURFACE_SELECTOR.viewer);
    // Rescue restarted or removed workflow details before removing an empty history fold.
    if (!isCompleted && detail.parentElement !== viewer) {
      viewer.insertBefore(detail, host.querySelector(COMPLETED_FOLD_SELECTOR));
    }
    const completed = completedFoldContainer(groups.completed.length);
    const choices = (group) => group.map((row) => ({
      ...row,
      index: rows.indexOf(row),
      selected: row === workflow,
    }));
    paintList(host.querySelector(SURFACE_SELECTOR.workflowChoices), {
      rows: rows.length > 1 ? choices(groups.running) : [],
      render: workflowChoiceHtml,
    });
    if (completed) paintList(completed, { rows: choices(groups.completed), render: workflowChoiceHtml });

    const container = isCompleted ? completed.parentElement : viewer;
    if (detail.parentElement !== container) container.appendChild(detail);
    detail.hidden = !workflow;
    patchElement(detail.querySelector(WORKFLOW_HEAD_SELECTOR), el(workflowHeadHtml(workflow || {})));
    paintList(detail.querySelector(SURFACE_SELECTOR.workflowPhases), {
      rows: workflow ? workflowPhases(surfaces, selectedIndex, reading)
        .map((phase) => ({ ...phase, key: `${workflow.key}:${phase.key}` })) : [],
      render: phaseSectionHtml,
      onPainted: (section, phase) => openNewlyRunningPhase(openedPhases, section, phase),
      nested: (phase) => ({
        selector: SURFACE_SELECTOR.workflowAgents,
        rows: phase.rows,
        render: (row) => agentRowHtml(row, rowOptions),
      }),
    });
    if (justFinished) {
      historyOpen = true;
      completed.parentElement.open = true;
    }
  };

  const paint = () => {
    if (disposed || !foldKnown) return;
    const reading = { nowMs: Date.now(), modelLabel };
    const paintContext = { surfaces, reading, rowOptions, openedPhases };
    if (kind === WORKFLOW_ENTRY_KIND) {
      paintWorkflow(reading);
    } else {
      if (plan.headSelector) {
        patchElement(host.querySelector(plan.headSelector), el(plan.headHtml(paintContext)));
      }
      for (const list of plan.lists(paintContext)) {
        const container = list.folded ? completedFoldContainer(list.rows.length) : host.querySelector(list.selector);
        if (!container) continue;
        paintList(container, list);
      }
    }
    syncHistoryControl();
    applyCachedFolds();
    tickWhileAnyRowIsRunning();
  };

  const onHistoryPress = () => {
    const completed = host.querySelector(COMPLETED_FOLD_SELECTOR);
    if (!completed) return;
    if (foldRecord) {
      if (historyOpen) {
        const closed = { ...foldState, history: false };
        for (const row of completed.querySelectorAll("details.surface-agent[open]")) closed[`agent:${row.dataset.key}`] = false;
        void foldRecord.write(closed);
      } else {
        writeFold("history", true);
      }
      return;
    }
    setHistoryOpen(!historyOpen, true);
  };

  const onViewerPress = (event) => {
    if (expandClippedText(event)) return;
    const workflow = event.target.closest("[data-workflow-index]");
    if (workflow) {
      const chosen = openWorkflow(surfaces, Number(workflow.dataset.workflowIndex))?.key ?? null;
      if (foldRecord) {
        void foldRecord.write({ ...foldState, selectedWorkflowKey: chosen });
        return;
      }
      selectedWorkflowKey = chosen;
      selectedWorkflowWasRunning = false;
      paint();
      return;
    }
    const spawned = event.target.closest("[data-call-sequence]");
    if (spawned && onOpenThreadItem) {
      event.preventDefault();
      onOpenThreadItem(Number(spawned.dataset.callSequence));
    }
  };

  const onViewerKey = (event) => {
    if (CLIP_KEYS.includes(event.key)) expandClippedText(event);
  };

  const onViewerToggle = (event) => {
    const key = foldKey(event.target);
    if (foldRecord && key) writeFold(key, event.target.open);
    const agent = event.target.closest("details.surface-agent");
    if (agent) {
      if (agent.open) openedAgentKeys.add(agent.dataset.key);
      else openedAgentKeys.delete(agent.dataset.key);
      return;
    }
    const completed = event.target.closest(COMPLETED_FOLD_SELECTOR);
    if (completed) {
      if (!historyControl) historyOpen = completed.open;
      syncHistoryControl();
      if (!completed.open) {
        for (const row of completed.querySelectorAll("details.surface-agent[open]")) {
          openedAgentKeys.delete(row.dataset.key);
          row.open = false;
        }
      }
    }
  };

  host.innerHTML = plan.frameHtmlWithEmptyLists();
  host.addEventListener("click", onViewerPress);
  host.addEventListener("keydown", onViewerKey);
  host.addEventListener("toggle", onViewerToggle, true);
  historyControl?.addEventListener("click", onHistoryPress);
  syncHistoryControl(0);
  if (cacheKey) {
    foldRecord = watchUiState(uiAddress({ entityId: cacheKey, view: "surface-viewer", kind: "fold", sub: kind }), (saved) => {
      foldState = saved && typeof saved === "object" ? saved : {};
      autoHistoryPending = false;
      selectedWorkflowKey = typeof foldState.selectedWorkflowKey === "string" ? foldState.selectedWorkflowKey : null;
      openedAgentKeys.clear();
      openedPhases.clear();
      for (const [name, open] of Object.entries(foldState)) {
        if (name.startsWith("agent:") && open) openedAgentKeys.add(name.slice(6));
        if (name.startsWith("phase:")) openedPhases.add(name.slice(6));
      }
      foldKnown = true;
      paint();
    });
    void foldRecord.ready.then(() => {
      if (disposed || foldKnown) return;
      foldKnown = true;
      paint();
    });
  }

  return {
    kind,
    ready: foldRecord?.ready || Promise.resolve(),
    set(nextSurfaces) {
      surfaces = nextSurfaces || null;
      paint();
    },
    dispose() {
      disposed = true;
      stopTicking();
      foldRecord?.dispose({ flushPending: false });
      host.removeEventListener("click", onViewerPress);
      host.removeEventListener("keydown", onViewerKey);
      host.removeEventListener("toggle", onViewerToggle, true);
      historyControl?.removeEventListener("click", onHistoryPress);
      host.innerHTML = "";
    },
  };
}

export function openSurfaceOverlay(kind, { onOpenThreadItem, modelLabel, cacheKey = null, onClose = null, host = document.body }) {
  let viewer = null;
  const { body, close } = openModal({
    dialogHtml: surfaceOverlayHtml(surfaceKindLabel(kind)),
    host,
    onClose: () => {
      viewer.dispose();
      if (onClose) onClose();
    },
  });
  viewer = mountSurfaceViewer(body.querySelector(SURFACE_OVERLAY_BODY_SELECTOR), kind, { onOpenThreadItem, modelLabel, cacheKey });
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
  let chosenKind = null;
  let viewer = null;
  let closingFrame = null;
  let visibility = emptySurfaceVisibility();
  let hidingTimer = null;
  const disposeClearance = mountSurfaceClearance(viewerHost);

  const openKind = () => visibility.openKind;

  const closeOpenSurface = ({ restoreFocus = false } = {}) => {
    const openButton = pillHost.querySelector('[aria-pressed="true"]');
    void writeChoice(null).then(() => {
      if (restoreFocus && openButton?.isConnected) openButton.focus();
    });
  };

  const viewerCanvas = (kind) => {
    let canvas = viewerHost.querySelector(".surface-popover-body");
    if (!canvas) {
      viewerHost.innerHTML = `<div class="surface-popover-head">
        <strong class="surface-popover-kind"></strong>
        <span class="surface-popover-actions">
          <button type="button" class="surface-history-toggle" aria-pressed="false" hidden>
            ${ICON_HISTORY}<span class="surface-history-count"></span>
          </button>
          <button type="button" class="surface-popover-close">${ICON_X}</button>
        </span>
      </div><div class="surface-popover-body"></div>`;
      viewerHost.querySelector(".surface-popover-close").onclick = () => closeOpenSurface({ restoreFocus: true });
      canvas = viewerHost.querySelector(".surface-popover-body");
    }
    const label = surfaceKindLabel(kind);
    viewerHost.querySelector(".surface-popover-kind").textContent = label;
    const closeButton = viewerHost.querySelector(".surface-popover-close");
    closeButton.setAttribute("aria-label", `Close ${label}`);
    closeButton.title = `Close ${label}`;
    return canvas;
  };

  const closeViewerFrame = () => {
    if (!viewer) return;
    const frame = { viewer };
    closingFrame = frame;
    hide(viewerHost, { axis: "height" }).then(() => {
      if (closingFrame !== frame) return;
      closingFrame = null;
      viewer = null;
      frame.viewer.dispose();
      viewerHost.innerHTML = "";
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
    const canvas = viewerCanvas(kind);
    viewer = mountSurfaceViewer(canvas, kind, {
      onOpenThreadItem,
      modelLabel,
      compact: true,
      cacheKey: key,
      historyControl: viewerHost.querySelector(".surface-history-toggle"),
    });
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
    painted.forEach((element, index) => paintPillCount(element, pills[index].progress ?? pills[index].count));
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

  const choiceRecord = watchUiState(
    uiAddress({ entityId: key, view: "agent-surfaces", kind: "menu" }),
    (saved) => {
      chosenKind = typeof saved?.kind === "string" ? saved.kind : null;
      paint();
    },
  );
  let paintCommit = Promise.resolve();
  const writeChoice = (kind) => {
    paintCommit = choiceRecord.write({ kind });
    return paintCommit;
  };

  const onPillPress = (event) => {
    const button = event.target.closest("[data-surface-kind]");
    if (!button) return;
    const kind = button.dataset.surfaceKind;
    if (kind === openKind()) return closeOpenSurface();
    void writeChoice(kind);
  };

  const onEscape = (event) => {
    const nestedPopoverIsOpen = [...viewerHost.closest(".rail-panel")?.querySelectorAll(".splitmenu") || []]
      .some((menu) => !menu.hidden);
    if (event.defaultPrevented || event.key !== "Escape" || nestedPopoverIsOpen || !openKind()) return;
    event.preventDefault();
    closeOpenSurface({ restoreFocus: true });
  };

  pillHost.addEventListener("click", onPillPress);
  document.addEventListener("keydown", onEscape);

  return {
    ready: choiceRecord.ready,
    settled: () => paintCommit,
    set(nextSurfaces, seenAtMs = Date.now()) {
      const arriving = JSON.stringify(nextSurfaces || null);
      const unchangedSinceLastPaint = arriving === paintedSurfaces;
      paintedSurfaces = arriving;
      surfaces = nextSurfaces || null;
      visibility = advanceSurfaceVisibility(visibility, surfaces, seenAtMs);
      if (unchangedSinceLastPaint) return;
      paint();
    },
    dispose() {
      choiceRecord.dispose({ flushPending: false });
      if (hidingTimer !== null) clearTimeout(hidingTimer);
      hidingTimer = null;
      closingFrame = null;
      if (viewer) viewer.dispose();
      viewer = null;
      settleHidden(viewerHost);
      viewerHost.innerHTML = "";
      pillHost.removeEventListener("click", onPillPress);
      document.removeEventListener("keydown", onEscape);
      paintedSurfaces = null;
      disposeClearance();
      pillHost.innerHTML = "";
      notifyPillsChanged();
    },
  };
}
