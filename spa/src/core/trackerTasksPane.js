// The Tasks tab, mounted: the list and the board over one set of tasks, the
// filters that narrow them, and the two presses that change something.
//
// Cache-first, like every other surface here. The project's whole list is on
// disk (core/trackerCache.js, kept warm by core/trackerSync.js on every pass),
// so the tab paints before the bridge is asked — and the filters are applied
// locally to that paint while the same filters go out as `tasks.list` params.
// Two readings of one rule; core/trackerFilters.js holds both.
//
// The push is the project's own: `{scope:{kind:"entity",id:project_id},
// kinds:["tasks"]}`. An item says which tasks moved and nothing more, so the
// answer is to read the list again.

import { messageOf } from "./text.js";
import { hashFromRoute } from "./router.js";
import { watchChanges } from "./changeEvents.js";
import { tasksPushKinds } from "./trackerPush.js";
import { notifyError } from "./notify.js";
import {
  tasksAddress,
  tasksQueryAddress,
  tasksRecord,
  listAskedAt,
  readTasksCached,
  readTasksQueryCached,
  writeTasksQueryRecord,
  writeTasksRecord,
} from "./trackerCache.js";
import { subscribeCache } from "./localCache.js";
import { foldTasksPage, pagesTasks, pullTaskPages } from "./trackerPages.js";
import { nextTaskRead, noteWritten } from "./taskReadOrder.js";
import { createReadRetry } from "./transientRead.js";
import { trailingRead } from "./trailingRead.js";
import { deviceSession, deviceWatch } from "./deviceReconnect.js";
import {
  DEFAULT_FILTERS,
  filterTasks,
  filterOptions,
  taskListParams,
  narrowsTheRead,
  sortTasks,
} from "./trackerFilters.js";
import { columnsOf } from "./trackerModel.js";
import { actorName } from "./trackerLineWords.js";
import { boardColumns, moveParams, nextColumn, withMovedTask } from "./trackerBoardModel.js";
import { agentLabels, assigneeOptions, projectName, selectedOptionId, workspaceAgents } from "./trackerAssignee.js";
import { BOARD_VIEW, DASHBOARD_VIEW, LIST_VIEW, mountTasksChrome } from "./trackerPaneChrome.js";
import { paintGroupedTaskRows, paintTaskBoard } from "./trackerTasksBody.js";
import { attentionGroups, NEEDS_YOU_GROUP, REST_GROUP, WORKING_GROUP } from "./trackerAttentionModel.js";
import { dashboardSections, doneSessionStart, doneSinceCutoff } from "./trackerDashboardModel.js";
import { readUserSession, userSessionAddress, writeListedUserSession } from "./userSessionCache.js";
import { needsYouRuleAddress, readNeedsYouRule } from "./needsYouRule.js";
import { DEFAULT_DASHBOARD_TAB, dashboardTabIds, paintTaskDashboard } from "./trackerDashboardRender.js";
import { createTrackerTaskDetailsFeed } from "./trackerTaskDetailsFeed.js";
import { taskUnreadCount } from "./taskUnread.js";
import { createTrackerAgentActivityFeed } from "./trackerAgentActivityFeed.js";
import { openAssigneePicker } from "./trackerAssigneePicker.js";
import { openTaskComposer } from "./taskComposer.js";
import { carriesTaskAttachments } from "./taskAttachments.js";
import { labelsOf } from "./trackerFilters.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { subscribeReferenceIndex } from "./referenceIndex.js";

export const TASK_PAGE_SIZE = 25;
const VIEW_IDS = new Set([DASHBOARD_VIEW, LIST_VIEW, BOARD_VIEW]);

export function mountTasksPane(host, options) {
  const initialView = VIEW_IDS.has(options.view) ? options.view : options.defaultView || DASHBOARD_VIEW;
  const state = {
    ...options,
    unscoped: [], // the whole list before a workspace's live roster narrows it
    unscopedShown: [], // the active filter-addressed projection before that roster narrows it
    all: [], // the project's whole list, which the filter menus are built from
    shown: [], // the narrowed list, which is what is painted
    visibleCount: TASK_PAGE_SIZE,
    collapsedGroups: new Set(),
    dashboardTab: DEFAULT_DASHBOARD_TAB,
    columns: [],
    // Open, not everything (#33): a closed task is done, and done work is not
    // what the tab is for. It is still one press away on the state filter.
    filters: { ...DEFAULT_FILTERS, state: initialView === DASHBOARD_VIEW ? "" : DEFAULT_FILTERS.state },
    stateFilterTouched: false,
    view: initialView,
    disposed: false,
    picker: null,
    composer: null,
    focusTask: null,
    // When the bridge was asked for each list this pane can paint from
    // (#119, #129).
    queryAsked: 0,
    wholeAsked: 0,
    userSession: null, // the bridge's, from the device's cache
    askedOnly: false, // the device's cached Needs you rule (#144)
  };
  const uiScope = { deviceId: state.deviceId, entityId: state.projectId, view: `tasks:${state.projectKey || "project"}` };
  const uiSnapshot = () => ({
    view: state.view,
    filters: state.filters,
    stateFilterTouched: state.stateFilterTouched,
    collapsedGroups: [...state.collapsedGroups],
    dashboardTab: state.dashboardTab,
    visibleCount: state.visibleCount,
  });
  let uiRecord;
  const saveUi = () => { if (uiRecord) void uiRecord.write(uiSnapshot()); };

  // ---- what the feed knows about this project's agents ---------------------

  const groups = () => workspaceAgents(state.feed(), state.projectKey);
  const labelsOfAgents = () => agentLabels(groups());
  /** What every row on this tab names an actor from: the project's agents, and
   *  the project itself, which its own agent is named after (#63). */
  const reading = () => ({ agentLabels: labelsOfAgents(), projectName: projectName(state.feed(), state.projectKey) });
  const nameActor = (actor) => actorName(actor, reading());

  /**
   * Which tasks this tab is about.
   *
   * The project's own Tasks tab is about all of them and passes nothing. A
   * workspace's tab (#29) passes the agents standing in it, and the narrowing
   * is done HERE rather than on the wire: `tasks.list` takes one assignee and
   * a workspace has several, so one pass over the list the tab already holds
   * answers for every agent at once.
   */
  const kept = (tasks) => (state.only ? (tasks || []).filter((task) => state.only(task)) : tasks || []);
  const rescope = () => {
    state.all = kept(state.unscoped);
    state.shown = kept(state.unscopedShown);
  };
  const previewQuery = () => {
    state.unscopedShown = filterTasks(state.unscoped, shownFilters());
    rescope();
    details?.updateTasks(state.shown);
  };

  /** Where one task opens. The project's tab opens the tracker's own page;
   *  a workspace's opens the same page INSIDE the workspace, because leaving
   *  the workspace to read a task is leaving the agents holding it. */
  const routeOf = (task) =>
    state.taskRoute
      ? state.taskRoute(task)
      : { name: "trackerTask", projectId: state.projectId, deviceId: state.deviceId, taskId: task.id };

  const hrefOf = (task) => hashFromRoute(routeOf(task));

  /** What the tab does when a read fails because the wire went away rather
   *  than because the bridge said no: keeps the list that is already on screen,
   *  marks when it was read, and reads again when the machine is back. The
   *  list and the board share it — they are two drawings of one read. */
  const reads = createReadRetry({
    host,
    watch: deviceWatch(state.deviceId),
    retry: () => void refresh(),
    hasContent: () => state.shown.length > 0 || state.all.length > 0,
  });

  // ---- painting ------------------------------------------------------------

  // Once. The header and the four filters are made here and are the same DOM
  // nodes for the life of the pane; only the body below them is ever repainted
  // (#43). Nothing that follows can take the reader's focus, their caret or
  // the menu they have open, because nothing that follows touches a control.
  const chrome = mountTasksChrome(host, {
    scopeLink: state.scopeLink || null,
    menuAddressOf: (name) => uiAddress({ ...uiScope, kind: "menu", sub: name }),
    onView: (view) => {
      if (state.view === view) return;
      state.view = view;
      if (!state.stateFilterTouched) state.filters.state = view === DASHBOARD_VIEW ? "" : DEFAULT_FILTERS.state;
      state.visibleCount = TASK_PAGE_SIZE;
      saveUi();
    },
    onNew: () => fileTask(),
    onFilter: (name, chosen) => {
      state.filters = { ...state.filters, [name]: chosen };
      if (name === "state") state.stateFilterTouched = true;
      state.visibleCount = TASK_PAGE_SIZE;
      saveUi();
    },
    onClear: () => {
      // Back to what the tab opens on, not to everything: Clear undoes the
      // reader's narrowing, and closed tasks were never part of it.
      state.filters = { ...DEFAULT_FILTERS, state: state.view === DASHBOARD_VIEW ? "" : DEFAULT_FILTERS.state };
      state.stateFilterTouched = false;
      state.visibleCount = TASK_PAGE_SIZE;
      saveUi();
    },
  });

  const paintContext = () => ({
    columns: state.columns,
    unreadOf: unreadCounter(),
    deviceId: state.deviceId,
    projectId: state.projectId,
    agentGroups: groups(),
    workspaces: (state.feed()?.workspaces || []).filter((workspace) => workspace.projectKey === state.projectKey),
    ...reading(),
    filters: state.filters,
    href: hrefOf,
    paging: {
      total: state.shown.length,
      more: () => {
        state.visibleCount += TASK_PAGE_SIZE;
        saveUi();
      },
    },
    onToggleGroup: (id) => {
      if (state.collapsedGroups.has(id)) state.collapsedGroups.delete(id);
      else state.collapsedGroups.add(id);
      saveUi();
    },
    dashboardTab: state.dashboardTab,
    doneSinceLeft: carriesDoneSinceLeft(),
    onDashboardTab: (id) => {
      if (!dashboardTabIds.includes(id) || state.dashboardTab === id) return;
      state.dashboardTab = id;
      saveUi();
    },
  });

  /** Whether this device's bridge carries `done_at` and the user's session.
   *  Answered by the cache, not the greeting (#104 review): only a bridge that
   *  carries it sends the session this device holds, and a list from one that
   *  does not drops it (`writeListedUserSession`), so a cold or offline start
   *  paints the Done it will keep. */
  function carriesDoneSinceLeft() {
    return state.userSession !== null;
  }

  const groupLabels = [
    [WORKING_GROUP, "In progress with an agent"],
    [NEEDS_YOU_GROUP, "Needs you"],
    [REST_GROUP, "Other tasks"],
  ];
  let details;
  let activity;
  /** Each watched task's unread, as the bubble on its row, card or dashboard
   *  line says it (#104): off the cached timeline while it is current, and
   *  the list's own count otherwise (core/taskUnread.js). */
  function unreadCounter() {
    const detailById = details?.read() || new Map();
    return (task) => taskUnreadCount(task, detailById.get(task.id) || null);
  }
  const groupedRows = () => {
    const attention = attentionGroups(state.shown, {
      feed: state.feed(), projectKey: state.projectKey, detailById: details?.read(), askedOnly: state.askedOnly,
    });
    let remaining = state.visibleCount;
    return groupLabels.map(([id, title]) => {
      const tasks = attention[id];
      const page = tasks.slice(0, remaining);
      remaining -= page.length;
      return { id, title, tasks: page, count: tasks.length, collapsed: state.collapsedGroups.has(id) };
    });
  };

  /** The two drawings of one read, each as what it paints, what it paints from
   *  and what has to be wired onto an entry it had to make. Chosen by name
   *  rather than asked about: a view is a thing this tab HAS, not a branch. */
  const VIEWS = {
    [DASHBOARD_VIEW]: {
      paint: paintTaskDashboard,
      entries: () => dashboardSections(state.shown, {
        feed: state.feed(), projectKey: state.projectKey, detailById: details.read(),
        activityByAgent: activity?.read(),
        doneCutoffMs: carriesDoneSinceLeft() ? doneSinceCutoff(state.userSession) : null,
        sessionStartedMs: carriesDoneSinceLeft() ? doneSessionStart(state.userSession) : null,
        askedOnly: state.askedOnly,
        columns: state.columns,
      }),
    },
    [LIST_VIEW]: { paint: paintGroupedTaskRows, entries: groupedRows, wire: wireRow },
    [BOARD_VIEW]: { paint: paintTaskBoard, entries: () => boardColumns(state.columns, state.shown), wire: wireCard },
  };

  const paint = () => {
    if (state.disposed) return;
    chrome.update({
      view: state.view,
      // The menus are built from the project's WHOLE list and from the feed:
      // the labels its tasks wear, and every agent standing on one of its
      // workspaces, grouped the way the assignee picker groups them (#44).
      options: filterOptions(state.all, state.columns, nameActor, groups()),
      filters: state.filters,
    });
    const view = VIEWS[state.view] || VIEWS[LIST_VIEW];
    view.paint(chrome.body, view.entries(), paintContext(), view.wire);
    wireColumnDrops();
    focusPendingTask();
  };

  uiRecord = watchUiState(uiAddress({ ...uiScope, kind: "filters" }), (saved) => {
    if (state.disposed || !saved) return;
    const priorView = state.view;
    state.view = VIEW_IDS.has(saved.view) ? saved.view : state.view;
    state.filters = { ...state.filters, ...(saved.filters || {}) };
    state.stateFilterTouched = Boolean(saved.stateFilterTouched);
    state.collapsedGroups = new Set(saved.collapsedGroups || []);
    state.dashboardTab = dashboardTabIds.includes(saved.dashboardTab) ? saved.dashboardTab : DEFAULT_DASHBOARD_TAB;
    state.visibleCount = Number(saved.visibleCount) || TASK_PAGE_SIZE;
    if (priorView !== state.view) state.onViewChange?.(state.view);
    previewQuery();
    paint();
    watchQuery();
    void refresh();
  });

  function focusPendingTask() {
    if (!state.focusTask) return;
    const taskId = state.focusTask;
    const focused = state.view === BOARD_VIEW ? focusCard(taskId)
      : state.view === DASHBOARD_VIEW ? focusDashboardRow(taskId) : focusRow(taskId);
    if (focused) state.focusTask = null;
  }

  /** The active wire read has its own cache record. It is separate from the
   *  project's whole list so a filtered answer can never empty the menus. */
  const queryParams = () => taskListParams(state.projectId, shownFilters());
  let queryUnsubscribe = null;
  let querySerial = 0;
  let queryLoaded = false;

  /** Whether the whole list was asked for no earlier than the filtered answer.
   *  The pass behind this tab (core/cacheSync.js) pulls only the whole list, so
   *  after a gap no push described, it is the newer news about these same
   *  tasks and the filters are applied to it locally, as before the first
   *  answer (#119). By when each was asked, not when each landed: that pass's
   *  read is large, and one asked before this tab's small one can land after
   *  it still the older news (#129). */
  const wholeListIsNewer = () => state.wholeAsked >= state.queryAsked;

  async function paintFromQuery(params, serial = querySerial) {
    const cached = await readTasksQueryCached(state.deviceId, state.projectId, params);
    if (state.disposed || serial !== querySerial || !cached) return;
    const record = cached.value;
    queryLoaded = true;
    state.queryAsked = listAskedAt(cached);
    state.unscopedShown = wholeListIsNewer()
      ? filterTasks(state.unscoped, shownFilters())
      : sortTasks(record.tasks);
    state.shown = kept(state.unscopedShown);
    details?.updateTasks(state.shown);
    // On a cold device the background whole-list pass may not have landed
    // yet. Until it does, this cache record is still the only cache-derived
    // source from which the menus can be built.
    if (!state.unscoped.length) {
      state.unscoped = state.unscopedShown;
      state.all = kept(state.unscoped);
    }
    reads.succeeded();
    paint();
  }

  function watchQuery() {
    queryUnsubscribe?.();
    querySerial += 1;
    queryLoaded = false;
    const serial = querySerial;
    const params = queryParams();
    queryUnsubscribe = subscribeCache(tasksQueryAddress(state.deviceId, state.projectId, params), () => {
      void paintFromQuery(params, serial);
    });
    void paintFromQuery(params, serial);
  }

  // ---- reading -------------------------------------------------------------

  /** What the cache holds, painted before anything is asked. A project never
   *  opened on this device holds nothing, and the tab simply waits. */
  async function paintFromCache() {
    const cached = await readTasksCached(state.deviceId, state.projectId);
    if (state.disposed || !cached) return;
    const { at, value: record } = cached;
    state.unscoped = sortTasks(record.tasks);
    state.all = kept(state.unscoped);
    state.columns = columnsOf(record.columns);
    state.wholeAsked = listAskedAt(cached);
    if (!queryLoaded || wholeListIsNewer()) {
      state.unscopedShown = filterTasks(state.unscoped, shownFilters());
      state.shown = kept(state.unscopedShown);
    }
    details?.updateTasks(state.shown);
    reads.seen(at); // this list is as old as the cache's stamp, not as old as now
    paint();
  }

  const wholeListWatcher = subscribeCache(tasksAddress(state.deviceId, state.projectId), () => {
    void paintFromCache();
  });

  async function paintUserSession() {
    const session = await readUserSession(state.deviceId);
    if (state.disposed) return;
    state.userSession = session;
    if (state.view === DASHBOARD_VIEW) paint();
  }
  const userSessionWatcher = subscribeCache(userSessionAddress(state.deviceId), () => {
    void paintUserSession();
  });

  /** Which Needs you rule this machine's tasks are read by. Read before the
   *  first paint, so a cold open draws the Needs you it will keep. */
  async function readRule() {
    const askedOnly = await readNeedsYouRule(state.deviceId);
    if (state.disposed || askedOnly === state.askedOnly) return false;
    state.askedOnly = askedOnly;
    return true;
  }
  const ruleWatcher = subscribeCache(needsYouRuleAddress(state.deviceId), () => {
    void readRule().then((changed) => changed && paint());
  });

  details = createTrackerTaskDetailsFeed({
    deviceId: state.deviceId,
    projectId: state.projectId,
    callRpc: state.callRpc,
    onChange: () => paint(),
  });
  activity = createTrackerAgentActivityFeed({ deviceId: state.deviceId, onChange: () => paint() });
  void activity.updateFeed(state.feed(), state.projectKey);
  // The Dashboard's activity line reads each reference as its words, off the
  // shared index, which can learn a name after the line is drawn.
  const referencesWatcher = subscribeReferenceIndex(() => paint());

  /** The board's columns ARE the statuses, so narrowing by one there would
   *  empty every other column rather than filter anything. The three filters
   *  that mean something on a board are sent; the status is not. */
  const shownFilters = () => (state.view === BOARD_VIEW ? { ...state.filters, status: "" } : state.filters);

  /** Read the list again. A busy project pushes every flush, so a read asked
   *  for while one is out waits for it and runs once after it (#119): every
   *  answer lands, and a push is never answered by a read begun before it. */
  let listReads = null;
  function refresh() {
    listReads ||= trailingRead(readList, { generationOf: () => deviceSession(state.deviceId) });
    return listReads();
  }

  async function readList() {
    if (state.disposed) return;
    const filters = shownFilters();
    try {
      const params = taskListParams(state.projectId, filters);
      if (pagesTasks(state.deviceId)) await readListPages(params, filters);
      else await readWholeList(params, filters);
    } catch (error) {
      if (state.disposed) return;
      // The wire going away is not news about this project's tasks. With a
      // list on screen the tab keeps it and waits; with nothing on screen it
      // waits too, and says so if the read still fails once the machine is back.
      if (reads.failed(error)) return;
      notifyError("Could not read this project's tasks", messageOf(error));
    }
  }

  async function readWholeList(params, filters) {
    const read = await nextTaskRead();
    const answer = await state.callRpc("tasks.list", params);
    if (state.disposed) return;
    const record = tasksRecord(answer?.tasks, state.columns, read);
    // The fetch is a writer only. The matching cache announcement above is
    // what re-reads this record and repaints the pane.
    await writeTasksQueryRecord(state.deviceId, state.projectId, params, record);
    // An unnarrowed answer is also the authoritative whole-list record.
    if (!narrowsTheRead(filters)) await writeTasksRecord(state.deviceId, state.projectId, record);
    await writeListedUserSession(state.deviceId, answer);
  }

  /** The same read a page at a time, from a bridge that pages it (#85). Each
   *  page is laid over the records the whole answer would have replaced, for
   *  the numbers it answers for, and the pane repaints from each. A walk
   *  belongs to the session it began on: once the device reconnects, a page
   *  that session asked for is neither written nor followed, and the new
   *  session's read (#119) walks the list again. */
  async function readListPages(params, filters) {
    const { deviceId, projectId } = state;
    const session = deviceSession(deviceId);
    const addresses = [tasksQueryAddress(deviceId, projectId, params)];
    if (!narrowsTheRead(filters)) addresses.push(tasksAddress(deviceId, projectId));
    await pullTaskPages({
      ask: (asked) => state.callRpc("tasks.list", asked),
      deviceId,
      projectId,
      params,
      active: () => !state.disposed && deviceSession(deviceId) === session,
      fold: async (stretch, page) => {
        const committed = await Promise.all([
          ...addresses.map((address) => foldTasksPage(address, stretch, () => state.columns)),
          writeListedUserSession(deviceId, page),
        ]);
        return committed.slice(0, addresses.length).every(Boolean);
      },
    });
  }

  // ---- moving a card -------------------------------------------------------

  /**
   * Move one task to one column.
   *
   * Optimistically: the card moves now and the column repaints from the push.
   * A refused move puts it back where it was and says why — a card that stayed
   * put with no word is a card the reader will drag again.
   */
  async function moveTask(taskId, status) {
    const task = state.shown.find((candidate) => candidate.id === taskId);
    if (!task || task.status === status) return;
    const heldQuery = state.unscopedShown;
    const heldCatalogue = state.unscoped;
    const movedQuery = withMovedTask(heldQuery, taskId, status);
    const movedCatalogue = withMovedTask(heldCatalogue, taskId, status);
    state.focusTask = taskId;
    const moved = [tasksQueryAddress(state.deviceId, state.projectId, queryParams()), tasksAddress(state.deviceId, state.projectId)];
    // Newer than any page still out, in this tab or another: one landing
    // after this does not put the card back (core/taskReadOrder.js), and
    // both lists carry that number, so neither reads as older than the other.
    const movedAs = await noteWritten(moved, [taskId]);
    await Promise.all([
      writeTasksQueryRecord(state.deviceId, state.projectId, queryParams(), tasksRecord(movedQuery, state.columns, movedAs)),
      writeTasksRecord(state.deviceId, state.projectId, tasksRecord(movedCatalogue, state.columns, movedAs)),
    ]);
    try {
      await state.callRpc("tasks.update", moveParams(taskId, status));
    } catch (error) {
      if (state.disposed) return;
      state.focusTask = taskId;
      const restoredAs = await noteWritten(moved, [taskId]);
      await Promise.all([
        writeTasksQueryRecord(state.deviceId, state.projectId, queryParams(), tasksRecord(heldQuery, state.columns, restoredAs)),
        writeTasksRecord(state.deviceId, state.projectId, tasksRecord(heldCatalogue, state.columns, restoredAs)),
      ]);
      notifyError("Could not move this task", messageOf(error));
    }
  }

  /** Keep the keyboard where it was. A repaint replaces the card that was
   *  focused, so the new one is found and focused again — by a scan rather than
   *  a selector, because an id is data and a selector is a language. */
  const focusCard = (taskId) => {
    if (state.disposed) return;
    for (const card of host.querySelectorAll(".task-card")) {
      if (card.dataset.task === taskId) {
        card.focus();
        return true;
      }
    }
    return false;
  };

  /** The keyboard's half of dragging. Left and right through the columns, and
   *  a stop at each end: wrapping from Done to Backlog is never what a repeated
   *  press meant. */
  function moveByKey(card, key) {
    const steps = key === "ArrowRight" ? 1 : -1;
    const column = nextColumn(state.columns, card.dataset.status, steps);
    if (column) void moveTask(card.dataset.task, column.id);
  }

  // ---- the two presses -----------------------------------------------------

  function openPicker(taskId) {
    const task = state.shown.find((candidate) => candidate.id === taskId);
    if (!task) return;
    state.picker = openAssigneePicker({
      task,
      options: assigneeOptions(groups()),
      current: selectedOptionId(task.assignee),
      catalog: state.catalog(),
      callRpc: state.callRpc,
      onAssigned: () => void refresh(),
    });
    void state.refreshCatalog?.().then((catalog) => state.picker?.setCatalog(catalog));
  }

  /** The task was filed and the assignee was not. Said after the fact rather
   *  than in the form, because the filing SUCCEEDED — keeping the form up
   *  would invite a second one — and said persistently, because the reader
   *  believes an agent is working and none is. */
  const sayTheAssigneeWasDropped = (task) =>
    notifyError(
      `Filed #${task?.number ?? ""} — but nobody was assigned`,
      "The Build on this machine cannot assign a task as it is filed, so it was created unassigned and nothing was started. Assign it from the task page.",
    );

  /** The task was filed and the files were not (#57). The same shape of
   *  silence and the same answer: the screenshot was usually the reason for
   *  filing, so "it is not there" is worth a sentence rather than a discovery. */
  const sayTheFilesWereDropped = (task, count) =>
    notifyError(
      `Filed #${task?.number ?? ""} — but ${count === 1 ? "the file" : `the ${count} files`} did not go with it`,
      "The Build on this machine cannot carry files on a task, so it was filed without them. Nothing was lost on your side — attach them to a comment once this machine can take them.",
    );

  /**
   * A freshly filed task, on screen before the read that confirms it.
   *
   * The list is keyed (core/trackerTasksBody.js), so this is one row inserted
   * rather than a repaint — which is the whole point of filing in place: you
   * see the thing you just wrote appear in the list you wrote it against. The
   * refresh behind it replaces this record with the bridge's own.
   */
  async function showTheNewTask(task) {
    if (!task || state.disposed) return;
    const held = kept([task]);
    if (!held.length) return; // a workspace's tab may not be about this task
    const catalogue = sortTasks([...state.unscoped.filter((one) => one.id !== task.id), task]);
    const query = sortTasks([...state.unscopedShown.filter((one) => one.id !== task.id), task]);
    state.focusTask = task.id;
    const filedAs = await nextTaskRead();
    await Promise.all([
      writeTasksRecord(state.deviceId, state.projectId, tasksRecord(catalogue, state.columns, filedAs)),
      writeTasksQueryRecord(state.deviceId, state.projectId, queryParams(), tasksRecord(query, state.columns, filedAs)),
    ]);
  }

  /** Put the keyboard on a row by the task it is about — by a scan rather
   *  than a selector, because an id is data and a selector is a language. */
  const focusRow = (taskId) => {
    for (const row of host.querySelectorAll(".task-row")) {
      if (row.dataset.task === taskId) {
        row.querySelector(".task-row-open")?.focus();
        return true;
      }
    }
    return false;
  };

  const focusDashboardRow = (taskId) => {
    for (const row of host.querySelectorAll(".task-dashboard-row")) {
      if (row.dataset.task === taskId) {
        row.querySelector(".task-dashboard-link")?.focus();
        return true;
      }
    }
    return false;
  };

  /**
   * Open the composer in place (#57).
   *
   * In the slot between the bar and the rows, not over them: a task is filed
   * ABOUT the list it is filed into, and a dialog that covers the list takes
   * away the one thing you were looking at while you wrote. A second press on
   * New task puts the focus back in the open one rather than opening another.
   */
  function fileTask() {
    if (state.composer?.isOpen()) {
      chrome.composeSlot.querySelector(".task-compose-title")?.focus();
      return;
    }
    state.composer = openTaskComposer(chrome.composeSlot, {
      projectId: state.projectId,
      deviceId: state.deviceId,
      projectName: state.projectName,
      columns: state.columns,
      labels: labelsOf(state.all),
      options: assigneeOptions(groups()),
      catalog: state.catalog(),
      // Asked as the form opens, not when the tab mounted: a greeting lands
      // after a tab is on screen, and a paperclip that waited for the next
      // navigation would be a capability nobody got the benefit of.
      attachable: carriesTaskAttachments(state.deviceId),
      callRpc: state.callRpc,
      onFiled: (answer, outcome) => {
        void showTheNewTask(answer?.task);
        void refresh();
        if (outcome?.assigneeWentNowhere) sayTheAssigneeWasDropped(answer?.task);
        if (outcome?.attachmentsWentNowhere) sayTheFilesWereDropped(answer?.task, outcome.attachmentCount);
      },
      onClosed: ({ filed }) => {
        state.composer = null;
        // Back where the reader was: on the row they just made, or on the
        // press that opened the form. Never nowhere, which is what closing a
        // focused subtree does if nobody says otherwise.
        if (!filed) host.querySelector("[data-task-new]")?.focus();
      },
    });
    void state.refreshCatalog?.();
  }

  // ---- wiring --------------------------------------------------------------

  // Once per element that had to be MADE. patchList keeps a row that is still
  // there, so a handler attached here survives every later paint; a handler
  // attached on a patch is a handler attached twice.

  /** Delegate from the retained row: gaining or losing an agent link changes
   *  the button's surrounding markup, so a patch can replace the button. */
  function wireRow(element) {
    element.onclick = (event) => {
      const assign = event.target.closest("[data-task-assign]");
      if (!assign || !element.contains(assign)) return;
      event.preventDefault();
      openPicker(assign.dataset.taskAssign);
    };
  }

  /** A card is a row that can also be moved — dragged, or walked left and
   *  right from the keyboard. Neither is the accessible afterthought of the
   *  other, so both are wired on every card. */
  function wireCard(card) {
    wireRow(card);
    card.ondragstart = (event) => event.dataTransfer?.setData("text/plain", card.dataset.task);
    card.onkeydown = (event) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      moveByKey(card, event.key);
    };
  }

  /** The other half of the drag: where a card can be let go of. The columns
   *  outlive their cards — the board's frame is patched, not rebuilt — so this
   *  runs per paint and writes the same two handlers onto the same lists. The
   *  list has none of these, and finds none. */
  function wireColumnDrops() {
    host.querySelectorAll("[data-column-drop]").forEach((column) => {
      column.ondragover = (event) => event.preventDefault();
      column.ondrop = (event) => {
        event.preventDefault();
        const taskId = event.dataTransfer?.getData("text/plain");
        if (taskId) void moveTask(taskId, column.dataset.columnDrop);
      };
    });
  }

  // ---- lifecycle -----------------------------------------------------------

  void Promise.all([uiRecord.ready, readRule()]).then(() => {
    if (state.disposed) return;
    paint();
    watchQuery();
    void paintUserSession();
    void paintFromCache().then(() => refresh());
  });
  // No cadence: nothing in this client polls. The tab hears that a task of
  // this project moved and reads the list again, and the pass behind it
  // (core/cacheSync.js) is the whole of the safety net.
  const watcher = watchChanges({
    refresh: () => void refresh(),
    entity: state.projectId,
    deviceId: state.deviceId,
  // Named only where the bridge carries them (core/trackerPush.js): every
  // kind in one subscribe shares that call's fate, and a refused one takes
  // this device's other subscriptions with it.
    kinds: tasksPushKinds(state.deviceId),
    mode: "realtime",
  });

  return {
    /** The feed moved: the agents a picker would offer may have, so the next
     *  paint names them again. Nothing is re-read from the bridge. */
    feedMoved() {
      rescope();
      details.updateTasks(state.shown);
      void activity.updateFeed(state.feed(), state.projectKey);
      paint();
    },
    dispose() {
      state.disposed = true;
      uiRecord.dispose();
      watcher.dispose();
      wholeListWatcher?.();
      userSessionWatcher?.();
      ruleWatcher?.();
      details.dispose();
      activity.dispose();
      referencesWatcher();
      queryUnsubscribe?.();
      reads.dispose();
      chrome.dispose();
      state.picker?.close?.();
      state.composer?.close?.();
    },
  };
}
