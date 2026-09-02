import { el } from "../dom.js";
import { patchElement } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { mountSplitMenu } from "./splitButton.js";
import { notifyError } from "./notify.js";
import {
  AGENT_ENTRY_KIND,
  CHECKLIST_ENTRY_KIND,
  SHELL_ENTRY_KIND,
  WORKFLOW_ENTRY_KIND,
  openSurfaceKind,
  openWorkflow,
  readOpenSurface,
  rowActions,
  surfacePills,
  surfaceRows,
  workflowChoices,
  workflowPhases,
  writeOpenSurface,
} from "./agentSurfacesModel.js";
import {
  SURFACE_LIST_SELECTOR,
  WORKFLOW_HEAD_SELECTOR,
  agentRowHtml,
  checklistItemHtml,
  checklistViewerHtml,
  shellRowHtml,
  shellViewerHtml,
  subagentViewerHtml,
  surfacePillsHtml,
  workflowChoiceHtml,
  workflowHeadHtml,
  workflowPhaseHtml,
  workflowViewerHtml,
} from "./agentSurfacesRender.js";

const VIEWER_ABOVE_PILLS_HTML = `<div class="rail-surfaces-viewer" data-surface-viewer></div>
  <div class="rail-surfaces-pills" data-surface-pills></div>`;

const NO_KIND_REMEMBERED = "";

const oneListOfKind = (kind, render, viewerHtml) => ({
  frameHtmlWithEmptyLists: () => viewerHtml([]),
  lists: ({ surfaces }) => [
    { selector: SURFACE_LIST_SELECTOR[kind], rows: surfaceRows(kind, surfaces), render, actionKind: kind },
  ],
});

const VIEWER_PLANS = {
  [WORKFLOW_ENTRY_KIND]: {
    frameHtmlWithEmptyLists: () => workflowViewerHtml({}, [], [], []),
    headSelector: WORKFLOW_HEAD_SELECTOR,
    headHtml: ({ workflow }) => workflowHeadHtml(workflow || {}),
    headActionKind: WORKFLOW_ENTRY_KIND,
    lists: ({ surfaces, workflow, selectedWorkflowIndex, selectedPhaseIndex }) => {
      const { phases, agents } = workflowPhases(workflow, selectedPhaseIndex);
      return [
        {
          selector: SURFACE_LIST_SELECTOR.workflowChoices,
          rows: workflowChoices(surfaces, selectedWorkflowIndex),
          render: workflowChoiceHtml,
        },
        { selector: SURFACE_LIST_SELECTOR.workflowPhases, rows: phases, render: workflowPhaseHtml },
        {
          selector: SURFACE_LIST_SELECTOR.workflowAgents,
          rows: agents,
          render: agentRowHtml,
          actionKind: AGENT_ENTRY_KIND,
        },
      ];
    },
  },
  [AGENT_ENTRY_KIND]: oneListOfKind(AGENT_ENTRY_KIND, agentRowHtml, subagentViewerHtml),
  [SHELL_ENTRY_KIND]: oneListOfKind(SHELL_ENTRY_KIND, shellRowHtml, shellViewerHtml),
  [CHECKLIST_ENTRY_KIND]: oneListOfKind(CHECKLIST_ENTRY_KIND, checklistItemHtml, checklistViewerHtml),
};

export function mountAgentSurfaces(host, { key, onSendMessage, onOpenThreadItem }) {
  host.innerHTML = VIEWER_ABOVE_PILLS_HTML;
  const viewerRegion = host.querySelector("[data-surface-viewer]");
  const pillRegion = host.querySelector("[data-surface-pills]");

  let surfaces = null;
  let chosenKind = readOpenSurface(key);
  let openKind = null;
  let paintedKind = null;
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

  const chooseRowAction = (actionKind, row, actionId) => {
    const action = rowActions(actionKind, row).find((candidate) => candidate.id === actionId);
    if (!action) throw new Error(`agentSurfaces: no action "${actionId}" on a ${actionKind} row`);
    sendRowMessage(action.message);
  };

  const rowUnderMenu = (element, selector) => {
    const painted = paintedLists.get(selector);
    return painted ? painted.rows.find((candidate) => candidate.key === element.dataset.key) : null;
  };

  const wireMenu = (element, chooseAction) => {
    const { closeMenu } = mountSplitMenu(element, { onChoose: chooseAction });
    menuClosersByElement.set(element, closeMenu);
  };

  const wireRowMenu = (element, selector, actionKind) => {
    wireMenu(element, (actionId) => {
      const row = rowUnderMenu(element, selector);
      if (!row) {
        notifyError("Could not ask the agent", "that row is gone");
        return;
      }
      chooseRowAction(actionKind, row, actionId);
    });
  };

  const closeMenusOfDiscardedElements = () => {
    for (const [element, closeMenu] of [...menuClosersByElement]) {
      if (viewerRegion.contains(element)) continue;
      closeMenu();
      menuClosersByElement.delete(element);
    }
  };

  const writeFrame = (plan) => {
    viewerRegion.innerHTML = plan.frameHtmlWithEmptyLists();
    paintedKind = openKind;
    closeMenusOfDiscardedElements();
    if (!plan.headSelector) return;
    wireMenu(viewerRegion.querySelector(plan.headSelector), (actionId) =>
      chooseRowAction(plan.headActionKind, openWorkflow(surfaces, selectedWorkflowIndex), actionId),
    );
  };

  const paintViewer = () => {
    paintedLists.clear();
    if (!openKind) {
      viewerRegion.innerHTML = "";
      paintedKind = null;
      closeMenusOfDiscardedElements();
      return;
    }
    const plan = VIEWER_PLANS[openKind];
    if (paintedKind !== openKind) writeFrame(plan);
    const paintContext = {
      surfaces,
      workflow: openWorkflow(surfaces, selectedWorkflowIndex),
      selectedWorkflowIndex,
      selectedPhaseIndex,
    };
    if (plan.headSelector) {
      patchElement(viewerRegion.querySelector(plan.headSelector), el(plan.headHtml(paintContext)));
    }
    for (const list of plan.lists(paintContext)) {
      paintedLists.set(list.selector, list);
      patchList(viewerRegion.querySelector(list.selector), list.rows, {
        keyOf: (row) => row.key,
        render: list.render,
        wire: list.actionKind ? (element) => wireRowMenu(element, list.selector, list.actionKind) : undefined,
      });
    }
    closeMenusOfDiscardedElements();
  };

  const paintPills = () => {
    const html = surfacePillsHtml(surfacePills(surfaces), openKind);
    const next = html ? el(html) : null;
    const live = pillRegion.firstElementChild;
    if (live && next && live.tagName === next.tagName) {
      patchElement(live, next);
      return;
    }
    pillRegion.innerHTML = html;
  };

  const paint = () => {
    openKind = openSurfaceKind(surfaces, chosenKind);
    paintViewer();
    paintPills();
  };

  const onPillPress = (event) => {
    const button = event.target.closest("[data-surface-kind]");
    if (!button) return;
    const kind = button.dataset.surfaceKind;
    chosenKind = kind === openKind ? null : kind;
    writeOpenSurface(key, chosenKind || NO_KIND_REMEMBERED);
    paint();
  };

  const onViewerPress = (event) => {
    if (event.target.closest(".splitbtn")) return;
    const workflow = event.target.closest("[data-workflow-index]");
    if (workflow) {
      selectedWorkflowIndex = Number(workflow.dataset.workflowIndex);
      selectedPhaseIndex = 0;
      paintViewer();
      return;
    }
    const phase = event.target.closest("[data-phase-index]");
    if (phase) {
      selectedPhaseIndex = Number(phase.dataset.phaseIndex);
      paintViewer();
      return;
    }
    const spawned = event.target.closest("[data-call-sequence]");
    if (spawned && onOpenThreadItem) onOpenThreadItem(Number(spawned.dataset.callSequence));
  };

  pillRegion.addEventListener("click", onPillPress);
  viewerRegion.addEventListener("click", onViewerPress);

  return {
    set(nextSurfaces) {
      surfaces = nextSurfaces || null;
      paint();
    },
    dispose() {
      for (const closeMenu of menuClosersByElement.values()) closeMenu();
      menuClosersByElement.clear();
      pillRegion.removeEventListener("click", onPillPress);
      viewerRegion.removeEventListener("click", onViewerPress);
      paintedLists.clear();
      host.innerHTML = "";
    },
  };
}
