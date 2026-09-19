// The Issues tab, mounted: the list and the board over one set of issues, the
// filters that narrow them, and the two presses that change something.
//
// Cache-first, like every other surface here. The project's whole list is on
// disk (core/trackerCache.js, kept warm by core/trackerSync.js on every pass),
// so the tab paints before the bridge is asked — and the filters are applied
// locally to that paint while the same filters go out as `issues.list` params.
// Two readings of one rule; core/trackerFilters.js holds both.
//
// The push is the project's own: `{scope:{kind:"entity",id:project_id},
// kinds:["issues"]}`. An item says which issues moved and nothing more, so the
// answer is to read the list again.

import { messageOf } from "./text.js";
import { hashFromRoute } from "./router.js";
import { watchChanges } from "./changeEvents.js";
import { notifyError } from "./notify.js";
import { ICON_PLUS } from "./icons.js";
import { readIssuesRecord } from "./trackerCache.js";
import { NO_FILTERS, filterIssues, filterOptions, filtersAreSet, issueListParams, sortIssues } from "./trackerFilters.js";
import { actorLabel, columnsOf } from "./trackerModel.js";
import { boardColumns, moveParams, nextColumn, withMovedIssue } from "./trackerBoardModel.js";
import { agentLabels, assigneeOptions, selectedOptionId, workspaceAgents } from "./trackerAssignee.js";
import { boardHtml } from "./trackerBoardRender.js";
import { filterBarHtml, issueListHtml } from "./trackerListRender.js";
import { openAssigneePicker } from "./trackerAssigneePicker.js";
import { openCreateIssue } from "./trackerCreate.js";

const LIST_VIEW = "list";
const BOARD_VIEW = "board";

/** The safety poll behind the push, which is what every mounted surface keeps.
 *  A subscription that is carrying re-times this to its own cadence. */
const REFRESH_MS = 20000;

const viewButtonHtml = (view, id, label) =>
  `<button class="btn mini issue-view${view === id ? " active" : ""}" type="button" data-issue-view="${id}" aria-pressed="${view === id}">${label}</button>`;

const headerHtml = (state) => `<div class="issue-head">
    <div class="issue-views" role="group" aria-label="How to lay the issues out">
      ${viewButtonHtml(state.view, LIST_VIEW, "List")}${viewButtonHtml(state.view, BOARD_VIEW, "Board")}
    </div>
    <button class="btn mini primary issue-new" type="button" data-issue-new>${ICON_PLUS}<span>New issue</span></button>
  </div>`;

export function mountIssuesPane(host, options) {
  const state = {
    ...options,
    all: [], // the project's whole list, which the filter menus are built from
    shown: [], // the narrowed list, which is what is painted
    columns: [],
    filters: { ...NO_FILTERS },
    view: options.view === BOARD_VIEW ? BOARD_VIEW : LIST_VIEW,
    disposed: false,
    picker: null,
  };

  // ---- what the feed knows about this project's agents ---------------------

  const groups = () => workspaceAgents(state.feed(), state.projectKey);
  const labelsOfAgents = () => agentLabels(groups());
  const nameActor = (actor) => actorLabel(actor, labelsOfAgents());

  const hrefOf = (issue) =>
    hashFromRoute({ name: "trackerIssue", projectId: state.projectId, deviceId: state.deviceId, issueId: issue.id });

  // ---- painting ------------------------------------------------------------

  const paintContext = () => ({
    columns: state.columns,
    agentLabels: labelsOfAgents(),
    filters: state.filters,
    href: hrefOf,
  });

  const bodyHtml = () => {
    const context = paintContext();
    if (state.view === BOARD_VIEW) return boardHtml(boardColumns(state.columns, state.shown), context);
    return issueListHtml(state.shown, context);
  };

  const paint = () => {
    if (state.disposed) return;
    host.innerHTML = `${headerHtml(state)}
      ${filterBarHtml(filterOptions(state.all, state.columns, nameActor), state.filters)}
      <div class="issue-body">${bodyHtml()}</div>`;
    wire();
  };

  // ---- reading -------------------------------------------------------------

  /** What the cache holds, painted before anything is asked. A project never
   *  opened on this device holds nothing, and the tab simply waits. */
  async function paintFromCache() {
    const record = await readIssuesRecord(state.deviceId, state.projectId);
    if (state.disposed || !record) return;
    state.all = sortIssues(record.issues);
    state.columns = columnsOf(record.columns);
    state.shown = filterIssues(state.all, shownFilters());
    paint();
  }

  /** The board's columns ARE the statuses, so narrowing by one there would
   *  empty every other column rather than filter anything. The three filters
   *  that mean something on a board are sent; the status is not. */
  const shownFilters = () => (state.view === BOARD_VIEW ? { ...state.filters, status: "" } : state.filters);

  async function refresh() {
    if (state.disposed) return;
    const filters = shownFilters();
    try {
      const answer = await state.callRpc("issues.list", issueListParams(state.projectId, filters));
      if (state.disposed) return;
      state.shown = sortIssues(answer?.issues);
      // An unnarrowed read IS the project's whole list; there is no second read
      // to make for it.
      if (!filtersAreSet(filters)) state.all = state.shown;
      await refreshWholeList(filtersAreSet(filters));
      paint();
    } catch (error) {
      if (!state.disposed) notifyError("Could not read this project's issues", messageOf(error));
    }
  }

  /** The filter menus are built from the project's WHOLE list, so choosing a
   *  label never empties the menu it was chosen from. A narrowed read is not
   *  that list, so the cache's copy stands in — the sync layer keeps it fresh
   *  on every pass, unnarrowed, for exactly this. The columns come from there
   *  either way: they are the project's, not any one read's. */
  async function refreshWholeList(narrowed) {
    const record = await readIssuesRecord(state.deviceId, state.projectId);
    if (state.disposed) return;
    if (narrowed && record) state.all = sortIssues(record.issues);
    if (record?.columns?.length) state.columns = columnsOf(record.columns);
  }

  // ---- moving a card -------------------------------------------------------

  /**
   * Move one issue to one column.
   *
   * Optimistically: the card moves now and the column repaints from the push.
   * A refused move puts it back where it was and says why — a card that stayed
   * put with no word is a card the reader will drag again.
   */
  async function moveIssue(issueId, status) {
    const held = state.shown;
    const issue = held.find((candidate) => candidate.id === issueId);
    if (!issue || issue.status === status) return;
    state.shown = withMovedIssue(held, issueId, status);
    paint();
    focusCard(issueId);
    try {
      await state.callRpc("issues.update", moveParams(issueId, status));
    } catch (error) {
      if (state.disposed) return;
      state.shown = held;
      paint();
      focusCard(issueId);
      notifyError("Could not move this issue", messageOf(error));
    }
  }

  /** Keep the keyboard where it was. A repaint replaces the card that was
   *  focused, so the new one is found and focused again — by a scan rather than
   *  a selector, because an id is data and a selector is a language. */
  const focusCard = (issueId) => {
    if (state.disposed) return;
    for (const card of host.querySelectorAll(".issue-card")) {
      if (card.dataset.issue === issueId) {
        card.focus();
        return;
      }
    }
  };

  /** The keyboard's half of dragging. Left and right through the columns, and
   *  a stop at each end: wrapping from Done to Backlog is never what a repeated
   *  press meant. */
  function moveByKey(card, key) {
    const steps = key === "ArrowRight" ? 1 : -1;
    const column = nextColumn(state.columns, card.dataset.status, steps);
    if (column) void moveIssue(card.dataset.issue, column.id);
  }

  // ---- the two presses -----------------------------------------------------

  function openPicker(issueId) {
    const issue = state.shown.find((candidate) => candidate.id === issueId);
    if (!issue) return;
    state.picker = openAssigneePicker({
      issue,
      options: assigneeOptions(groups()),
      current: selectedOptionId(issue.assignee),
      catalog: state.catalog(),
      callRpc: state.callRpc,
      onAssigned: () => void refresh(),
    });
    void state.refreshCatalog?.().then((catalog) => state.picker?.setCatalog(catalog));
  }

  function fileIssue() {
    openCreateIssue({
      projectId: state.projectId,
      projectName: state.projectName,
      options: assigneeOptions(groups()),
      catalog: state.catalog(),
      callRpc: state.callRpc,
      onFiled: (answer) => {
        void refresh();
        if (answer?.issue) state.navigate?.({ name: "trackerIssue", projectId: state.projectId, deviceId: state.deviceId, issueId: answer.issue.id });
      },
    });
  }

  // ---- wiring --------------------------------------------------------------

  function wireFilters() {
    host.querySelectorAll("[data-issue-filter]").forEach((control) => {
      control.onchange = () => {
        state.filters = { ...state.filters, [control.dataset.issueFilter]: control.value };
        state.shown = filterIssues(state.all, shownFilters());
        paint();
        void refresh();
      };
    });
    const clear = host.querySelector("[data-issue-filter-clear]");
    if (clear) {
      clear.onclick = () => {
        state.filters = { ...NO_FILTERS };
        paint();
        void refresh();
      };
    }
  }

  function wireViews() {
    host.querySelectorAll("[data-issue-view]").forEach((button) => {
      button.onclick = () => {
        if (state.view === button.dataset.issueView) return;
        state.view = button.dataset.issueView;
        state.onViewChange?.(state.view);
        paint();
        void refresh();
      };
    });
    host.querySelector("[data-issue-new]").onclick = fileIssue;
  }

  function wireDrag() {
    host.querySelectorAll(".issue-card").forEach((card) => {
      card.ondragstart = (event) => event.dataTransfer?.setData("text/plain", card.dataset.issue);
      card.onkeydown = (event) => {
        if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
        event.preventDefault();
        moveByKey(card, event.key);
      };
    });
    host.querySelectorAll("[data-column-drop]").forEach((column) => {
      column.ondragover = (event) => event.preventDefault();
      column.ondrop = (event) => {
        event.preventDefault();
        const issueId = event.dataTransfer?.getData("text/plain");
        if (issueId) void moveIssue(issueId, column.dataset.columnDrop);
      };
    });
  }

  function wire() {
    wireFilters();
    wireViews();
    wireDrag();
    host.querySelectorAll("[data-issue-assign]").forEach((button) => {
      button.onclick = (event) => {
        event.preventDefault();
        openPicker(button.dataset.issueAssign);
      };
    });
  }

  // ---- lifecycle -----------------------------------------------------------

  paint();
  void paintFromCache().then(() => refresh());
  const watcher = watchChanges({
    refresh: () => void refresh(),
    intervalMs: REFRESH_MS,
    entity: state.projectId,
    deviceId: state.deviceId,
    kinds: ["issues"],
  });

  return {
    /** The feed moved: the agents a picker would offer may have, so the next
     *  paint names them again. Nothing is re-read from the bridge. */
    feedMoved: paint,
    dispose() {
      state.disposed = true;
      watcher.dispose();
      state.picker?.close?.();
    },
  };
}
