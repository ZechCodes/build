// The agent's surfaces, mounted: the pill row under the conversation and the
// viewer that opens above it.
//
// This is the only file of the three that touches the DOM. It is handed a
// snapshot and paints it — the model answers what the rows are and the renderer
// writes their markup — and it wires the three gestures the reader has:
// toggling a pill, choosing a workflow phase, and asking the agent something
// about a row. It knows nothing about which entity it is standing on and it
// never reads the bridge: an action is an ordinary message, sent through the
// one send path the rail already owns.
//
// Every list in every viewer is painted with the keyed reconciler, keyed by the
// key the model put on the row and by nothing else. The workflow viewer is TWO
// of those lists — the phases and the selected phase's agents — so choosing a
// phase moves the highlight without rebuilding a single agent row.

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
  readOpenSurface,
  rowActions,
  surfacePills,
  surfaceRows,
  workflowPhases,
  writeOpenSurface,
} from "./agentSurfacesModel.js";
import {
  agentRowHtml,
  checklistItemHtml,
  checklistViewerHtml,
  shellRowHtml,
  shellViewerHtml,
  subagentViewerHtml,
  surfacePillsHtml,
  workflowPhaseHtml,
  workflowViewerHtml,
} from "./agentSurfacesRender.js";

/// The two regions, in the order the reader reads them: the viewer sits above
/// the pills, so opening one grows the block upward into the conversation
/// rather than pushing the box you write in off the bottom of the panel.
const REGIONS_HTML = `<div class="rail-surfaces-viewer" data-surface-viewer></div>
  <div class="rail-surfaces-pills" data-surface-pills></div>`;

const NO_KIND_REMEMBERED = "";

function firstElementOf(page, html) {
  const holder = page.createElement("template");
  holder.innerHTML = html;
  return holder.content.firstElementChild;
}

/// The workflow the viewer is showing. One at a time: the viewer draws one
/// workflow's phases and one phase's agents, and a session running two at once
/// is a shape the renderer has no second pane for.
function openWorkflowEntry(surfaces) {
  const workflows = surfaces && Array.isArray(surfaces.workflows) ? surfaces.workflows : [];
  return workflows.length ? workflows[0] : null;
}

/// What each kind's viewer is made of: the frame it is drawn in, painted with
/// its lists EMPTY, and the keyed lists that fill it.
///
/// The frame is written whole only when what it says changes, and everything
/// inside it is a keyed list — so a snapshot that moved a row leaves every
/// other row, and every fold the reader opened, exactly where it was.
const VIEWER_PLANS = {
  [WORKFLOW_ENTRY_KIND]: {
    frameHtml: (surfaces) => workflowViewerHtml(surfaceRows(WORKFLOW_ENTRY_KIND, surfaces)[0] || {}, [], []),
    lists: (surfaces, selectedPhaseIndex) => {
      const { phases, agents } = workflowPhases(openWorkflowEntry(surfaces), selectedPhaseIndex);
      return [
        { selector: ".surface-phases", rows: phases, render: workflowPhaseHtml },
        { selector: ".surface-phase-agents", rows: agents, render: agentRowHtml, actionKind: AGENT_ENTRY_KIND },
      ];
    },
  },
  [AGENT_ENTRY_KIND]: {
    frameHtml: () => subagentViewerHtml([]),
    lists: (surfaces) => [
      {
        selector: ".surface-subagents",
        rows: surfaceRows(AGENT_ENTRY_KIND, surfaces),
        render: agentRowHtml,
        actionKind: AGENT_ENTRY_KIND,
      },
    ],
  },
  [SHELL_ENTRY_KIND]: {
    frameHtml: () => shellViewerHtml([]),
    lists: (surfaces) => [
      {
        selector: ".surface-shells",
        rows: surfaceRows(SHELL_ENTRY_KIND, surfaces),
        render: shellRowHtml,
        actionKind: SHELL_ENTRY_KIND,
      },
    ],
  },
  [CHECKLIST_ENTRY_KIND]: {
    frameHtml: () => checklistViewerHtml([]),
    lists: (surfaces) => [
      {
        selector: ".surface-checklist",
        rows: surfaceRows(CHECKLIST_ENTRY_KIND, surfaces),
        render: checklistItemHtml,
        actionKind: CHECKLIST_ENTRY_KIND,
      },
    ],
  },
};

/// Mount the pills and the viewer into `host`.
///
/// `key` is the entity and the agent this block belongs to, which is what the
/// open pill is remembered against. `onSendMessage` is the rail's own send, so
/// a row action wakes and adopts exactly as a typed message does.
/// `onOpenThreadItem` is handed the thread sequence of the call that spawned a
/// subagent, and the conversation does the rest.
export function mountAgentSurfaces(host, { key, onSendMessage, onOpenThreadItem }) {
  host.innerHTML = REGIONS_HTML;
  const viewerRegion = host.querySelector("[data-surface-viewer]");
  const pillRegion = host.querySelector("[data-surface-pills]");

  let surfaces = null;
  // What the reader last chose, which is not the same as what is open: a kind
  // the snapshot has stopped carrying shows nothing, and shows itself again if
  // the agent starts one.
  let chosenKind = readOpenSurface(key);
  let openKind = null;
  let selectedPhaseIndex = 0;
  let paintedFrame = "";
  const paintedLists = new Map();
  const rowMenuClosers = [];

  const sendRowMessage = async (message) => {
    try {
      await onSendMessage(message);
    } catch (error) {
      notifyError("Could not ask the agent", error.message);
    }
  };

  const chooseRowAction = (actionKind, row, actionId) => {
    const action = rowActions(actionKind, row).find((candidate) => candidate.id === actionId);
    if (!action) return;
    sendRowMessage(action.message);
  };

  /// The menu on one row, wired once — patchList runs this only for an element
  /// it had to make, so the menu survives every later paint of the same row.
  /// Which row it is standing on is read back at press time, because the row
  /// under it is patched in place while the menu goes on hanging there.
  const wireRowMenu = (element, selector) => {
    if (!element.querySelector(".splitbtn")) return;
    const { closeMenu } = mountSplitMenu(element, {
      onChoose: (actionId) => {
        const painted = paintedLists.get(selector);
        const row = painted && painted.rows.find((candidate) => candidate.key === element.dataset.key);
        if (row) chooseRowAction(painted.actionKind, row, actionId);
      },
    });
    rowMenuClosers.push(closeMenu);
  };

  /// The menu on the workflow's own head, which is part of the frame rather
  /// than of a list. The frame carries the workflow's name, and the messages
  /// the menu offers are built out of that name — so a workflow whose name
  /// moved is a frame rewritten, and this is wired again with the row it now
  /// says.
  const wireFrameMenu = (workflowRow) => {
    const head = viewerRegion.querySelector(".surface-workflow-head");
    if (!head || !head.querySelector(".splitbtn")) return;
    const { closeMenu } = mountSplitMenu(head, {
      onChoose: (actionId) => chooseRowAction(WORKFLOW_ENTRY_KIND, workflowRow, actionId),
    });
    rowMenuClosers.push(closeMenu);
  };

  const paintViewer = () => {
    paintedLists.clear();
    if (!openKind) {
      viewerRegion.innerHTML = "";
      paintedFrame = "";
      return;
    }
    const plan = VIEWER_PLANS[openKind];
    const frame = plan.frameHtml(surfaces);
    if (paintedFrame !== frame) {
      viewerRegion.innerHTML = frame;
      paintedFrame = frame;
      wireFrameMenu(surfaceRows(WORKFLOW_ENTRY_KIND, surfaces)[0] || {});
    }
    for (const list of plan.lists(surfaces, selectedPhaseIndex)) {
      paintedLists.set(list.selector, list);
      patchList(viewerRegion.querySelector(list.selector), list.rows, {
        keyOf: (row) => row.key,
        render: list.render,
        wire: (element) => wireRowMenu(element, list.selector),
      });
    }
  };

  /// The pills, patched rather than rewritten: a pill that only changed which
  /// of them is pressed, or gained a live dot, is the same button afterwards —
  /// so a press that lands while the count is moving lands on the pill it was
  /// aimed at.
  const paintPills = () => {
    const html = surfacePillsHtml(surfacePills(surfaces), openKind);
    const next = html ? firstElementOf(pillRegion.ownerDocument, html) : null;
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
    // A press inside a row's menu is the menu's own; it has already said what
    // it means, and the row under it is not being opened.
    if (event.target.closest(".splitbtn")) return;
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
      rowMenuClosers.forEach((closeMenu) => closeMenu());
      rowMenuClosers.length = 0;
      pillRegion.removeEventListener("click", onPillPress);
      viewerRegion.removeEventListener("click", onViewerPress);
      paintedLists.clear();
      host.innerHTML = "";
    },
  };
}
