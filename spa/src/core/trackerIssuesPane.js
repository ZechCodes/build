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
import { bridgeCapabilities, watchChanges } from "./changeEvents.js";
import { issuesPushKinds } from "./trackerPush.js";
import { notifyError } from "./notify.js";
import {
  issuesAddress,
  issuesQueryAddress,
  issuesQueryRecordAt,
  issuesRecord,
  issuesRecordAt,
  readIssuesQueryRecord,
  readIssuesRecord,
  writeIssuesQueryRecord,
  writeIssuesRecord,
} from "./trackerCache.js";
import { subscribeCache } from "./localCache.js";
import { createReadRetry } from "./transientRead.js";
import { trailingRead } from "./trailingRead.js";
import { deviceSession, deviceWatch } from "./deviceReconnect.js";
import {
  DEFAULT_FILTERS,
  filterIssues,
  filterOptions,
  issueListParams,
  narrowsTheRead,
  sortIssues,
} from "./trackerFilters.js";
import { columnsOf } from "./trackerModel.js";
import { actorName } from "./trackerLineWords.js";
import { boardColumns, moveParams, nextColumn, withMovedIssue } from "./trackerBoardModel.js";
import { agentLabels, assigneeOptions, projectName, selectedOptionId, workspaceAgents } from "./trackerAssignee.js";
import { BOARD_VIEW, DASHBOARD_VIEW, LIST_VIEW, mountIssuesChrome } from "./trackerPaneChrome.js";
import { paintGroupedIssueRows, paintIssueBoard } from "./trackerIssuesBody.js";
import { attentionGroups, NEEDS_YOU_GROUP, REST_GROUP, WORKING_GROUP } from "./trackerAttentionModel.js";
import { dashboardSections, doneSinceCutoff } from "./trackerDashboardModel.js";
import { readUserSession, userSessionAddress, writeUserSession } from "./userSessionCache.js";
import { DEFAULT_DASHBOARD_TAB, dashboardTabIds, paintIssueDashboard } from "./trackerDashboardRender.js";
import { createTrackerIssueDetailsFeed } from "./trackerIssueDetailsFeed.js";
import { createTrackerAgentActivityFeed } from "./trackerAgentActivityFeed.js";
import { openAssigneePicker } from "./trackerAssigneePicker.js";
import { openIssueComposer } from "./issueComposer.js";
import { carriesIssueAttachments } from "./issueAttachments.js";
import { labelsOf } from "./trackerFilters.js";
import { uiAddress, watchUiState } from "./localUiState.js";

export const ISSUE_PAGE_SIZE = 25;
const VIEW_IDS = new Set([DASHBOARD_VIEW, LIST_VIEW, BOARD_VIEW]);

export function mountIssuesPane(host, options) {
  const initialView = VIEW_IDS.has(options.view) ? options.view : options.defaultView || DASHBOARD_VIEW;
  const state = {
    ...options,
    unscoped: [], // the whole list before a workspace's live roster narrows it
    unscopedShown: [], // the active filter-addressed projection before that roster narrows it
    all: [], // the project's whole list, which the filter menus are built from
    shown: [], // the narrowed list, which is what is painted
    visibleCount: ISSUE_PAGE_SIZE,
    collapsedGroups: new Set(),
    dashboardTab: DEFAULT_DASHBOARD_TAB,
    columns: [],
    // Open, not everything (#33): a closed issue is done, and done work is not
    // what the tab is for. It is still one press away on the state filter.
    filters: { ...DEFAULT_FILTERS, state: initialView === DASHBOARD_VIEW ? "" : DEFAULT_FILTERS.state },
    stateFilterTouched: false,
    view: initialView,
    disposed: false,
    picker: null,
    composer: null,
    focusIssue: null,
    // When the cache took each list this pane can paint from (#119).
    queryAt: 0,
    wholeAt: 0,
    userSession: null, // the bridge's, from the device's cache
  };
  const uiScope = { deviceId: state.deviceId, entityId: state.projectId, view: `issues:${state.projectKey || "project"}` };
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
   * Which issues this tab is about.
   *
   * The project's own Issues tab is about all of them and passes nothing. A
   * workspace's tab (#29) passes the agents standing in it, and the narrowing
   * is done HERE rather than on the wire: `issues.list` takes one assignee and
   * a workspace has several, so one pass over the list the tab already holds
   * answers for every agent at once.
   */
  const kept = (issues) => (state.only ? (issues || []).filter((issue) => state.only(issue)) : issues || []);
  const rescope = () => {
    state.all = kept(state.unscoped);
    state.shown = kept(state.unscopedShown);
  };
  const previewQuery = () => {
    state.unscopedShown = filterIssues(state.unscoped, shownFilters());
    rescope();
    details?.updateIssues(state.shown);
  };

  /** Where one issue opens. The project's tab opens the tracker's own page;
   *  a workspace's opens the same page INSIDE the workspace, because leaving
   *  the workspace to read an issue is leaving the agents holding it. */
  const routeOf = (issue) =>
    state.issueRoute
      ? state.issueRoute(issue)
      : { name: "trackerIssue", projectId: state.projectId, deviceId: state.deviceId, issueId: issue.id };

  const hrefOf = (issue) => hashFromRoute(routeOf(issue));

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
  const chrome = mountIssuesChrome(host, {
    scopeLink: state.scopeLink || null,
    menuAddressOf: (name) => uiAddress({ ...uiScope, kind: "menu", sub: name }),
    onView: (view) => {
      if (state.view === view) return;
      state.view = view;
      if (!state.stateFilterTouched) state.filters.state = view === DASHBOARD_VIEW ? "" : DEFAULT_FILTERS.state;
      state.visibleCount = ISSUE_PAGE_SIZE;
      saveUi();
    },
    onNew: () => fileIssue(),
    onFilter: (name, chosen) => {
      state.filters = { ...state.filters, [name]: chosen };
      if (name === "state") state.stateFilterTouched = true;
      state.visibleCount = ISSUE_PAGE_SIZE;
      saveUi();
    },
    onClear: () => {
      // Back to what the tab opens on, not to everything: Clear undoes the
      // reader's narrowing, and closed issues were never part of it.
      state.filters = { ...DEFAULT_FILTERS, state: state.view === DASHBOARD_VIEW ? "" : DEFAULT_FILTERS.state };
      state.stateFilterTouched = false;
      state.visibleCount = ISSUE_PAGE_SIZE;
      saveUi();
    },
  });

  const paintContext = () => ({
    columns: state.columns,
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
        state.visibleCount += ISSUE_PAGE_SIZE;
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
   *  Asked at each paint: the greeting can land after the cache has painted. */
  function carriesDoneSinceLeft() {
    return bridgeCapabilities(state.deviceId)?.issues?.doneSinceLeft === true;
  }

  const groupLabels = [
    [WORKING_GROUP, "In progress with an agent"],
    [NEEDS_YOU_GROUP, "Needs you"],
    [REST_GROUP, "Other issues"],
  ];
  let details;
  let activity;
  const groupedRows = () => {
    const attention = attentionGroups(state.shown, {
      feed: state.feed(), projectKey: state.projectKey, detailById: details?.read(),
    });
    let remaining = state.visibleCount;
    return groupLabels.map(([id, title]) => {
      const issues = attention[id];
      const page = issues.slice(0, remaining);
      remaining -= page.length;
      return { id, title, issues: page, count: issues.length, collapsed: state.collapsedGroups.has(id) };
    });
  };

  /** The two drawings of one read, each as what it paints, what it paints from
   *  and what has to be wired onto an entry it had to make. Chosen by name
   *  rather than asked about: a view is a thing this tab HAS, not a branch. */
  const VIEWS = {
    [DASHBOARD_VIEW]: {
      paint: paintIssueDashboard,
      entries: () => dashboardSections(state.shown, {
        feed: state.feed(), projectKey: state.projectKey, detailById: details.read(),
        activityByAgent: activity?.read(),
        doneCutoffMs: carriesDoneSinceLeft() ? doneSinceCutoff(state.userSession) : null,
      }),
    },
    [LIST_VIEW]: { paint: paintGroupedIssueRows, entries: groupedRows, wire: wireRow },
    [BOARD_VIEW]: { paint: paintIssueBoard, entries: () => boardColumns(state.columns, state.shown), wire: wireCard },
  };

  const paint = () => {
    if (state.disposed) return;
    chrome.update({
      view: state.view,
      // The menus are built from the project's WHOLE list and from the feed:
      // the labels its issues wear, and every agent standing on one of its
      // workspaces, grouped the way the assignee picker groups them (#44).
      options: filterOptions(state.all, state.columns, nameActor, groups()),
      filters: state.filters,
    });
    const view = VIEWS[state.view] || VIEWS[LIST_VIEW];
    view.paint(chrome.body, view.entries(), paintContext(), view.wire);
    wireColumnDrops();
    focusPendingIssue();
  };

  uiRecord = watchUiState(uiAddress({ ...uiScope, kind: "filters" }), (saved) => {
    if (state.disposed || !saved) return;
    const priorView = state.view;
    state.view = VIEW_IDS.has(saved.view) ? saved.view : state.view;
    state.filters = { ...state.filters, ...(saved.filters || {}) };
    state.stateFilterTouched = Boolean(saved.stateFilterTouched);
    state.collapsedGroups = new Set(saved.collapsedGroups || []);
    state.dashboardTab = dashboardTabIds.includes(saved.dashboardTab) ? saved.dashboardTab : DEFAULT_DASHBOARD_TAB;
    state.visibleCount = Number(saved.visibleCount) || ISSUE_PAGE_SIZE;
    if (priorView !== state.view) state.onViewChange?.(state.view);
    previewQuery();
    paint();
    watchQuery();
    void refresh();
  });

  function focusPendingIssue() {
    if (!state.focusIssue) return;
    const issueId = state.focusIssue;
    const focused = state.view === BOARD_VIEW ? focusCard(issueId)
      : state.view === DASHBOARD_VIEW ? focusDashboardRow(issueId) : focusRow(issueId);
    if (focused) state.focusIssue = null;
  }

  /** The active wire read has its own cache record. It is separate from the
   *  project's whole list so a filtered answer can never empty the menus. */
  const queryParams = () => issueListParams(state.projectId, shownFilters());
  let queryUnsubscribe = null;
  let querySerial = 0;
  let queryLoaded = false;

  /** Whether the whole list landed no earlier than the filtered answer. The pass
   *  behind this tab (core/cacheSync.js) pulls only the whole list, so after a
   *  gap no push described, it is the newer news about these same issues and
   *  the filters are applied to it locally, as before the first answer (#119). */
  const wholeListIsNewer = () => state.wholeAt >= state.queryAt;

  async function paintFromQuery(params, serial = querySerial) {
    const [record, at] = await Promise.all([
      readIssuesQueryRecord(state.deviceId, state.projectId, params),
      issuesQueryRecordAt(state.deviceId, state.projectId, params),
    ]);
    if (state.disposed || serial !== querySerial || !record) return;
    queryLoaded = true;
    state.queryAt = at;
    state.unscopedShown = wholeListIsNewer()
      ? filterIssues(state.unscoped, shownFilters())
      : sortIssues(record.issues);
    state.shown = kept(state.unscopedShown);
    details?.updateIssues(state.shown);
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
    queryUnsubscribe = subscribeCache(issuesQueryAddress(state.deviceId, state.projectId, params), () => {
      void paintFromQuery(params, serial);
    });
    void paintFromQuery(params, serial);
  }

  // ---- reading -------------------------------------------------------------

  /** What the cache holds, painted before anything is asked. A project never
   *  opened on this device holds nothing, and the tab simply waits. */
  async function paintFromCache() {
    const [record, at] = await Promise.all([
      readIssuesRecord(state.deviceId, state.projectId),
      issuesRecordAt(state.deviceId, state.projectId),
    ]);
    if (state.disposed || !record) return;
    state.unscoped = sortIssues(record.issues);
    state.all = kept(state.unscoped);
    state.columns = columnsOf(record.columns);
    state.wholeAt = at;
    if (!queryLoaded || wholeListIsNewer()) {
      state.unscopedShown = filterIssues(state.unscoped, shownFilters());
      state.shown = kept(state.unscopedShown);
    }
    details?.updateIssues(state.shown);
    reads.seen(at); // this list is as old as the cache's stamp, not as old as now
    paint();
  }

  const wholeListWatcher = subscribeCache(issuesAddress(state.deviceId, state.projectId), () => {
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

  details = createTrackerIssueDetailsFeed({
    deviceId: state.deviceId,
    projectId: state.projectId,
    callRpc: state.callRpc,
    onChange: () => paint(),
  });
  activity = createTrackerAgentActivityFeed({ deviceId: state.deviceId, onChange: () => paint() });
  void activity.updateFeed(state.feed(), state.projectKey);

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
      const params = issueListParams(state.projectId, filters);
      const answer = await state.callRpc("issues.list", params);
      if (state.disposed) return;
      const columns = state.columns;
      // The fetch is a writer only. The matching cache announcement above is
      // what re-reads this record and repaints the pane.
      await writeIssuesQueryRecord(state.deviceId, state.projectId, params, issuesRecord(answer?.issues, columns));
      // An unnarrowed answer is also the authoritative whole-list record.
      if (!narrowsTheRead(filters))
        await writeIssuesRecord(state.deviceId, state.projectId, issuesRecord(answer?.issues, columns));
      await writeUserSession(state.deviceId, answer);
    } catch (error) {
      if (state.disposed) return;
      // The wire going away is not news about this project's issues. With a
      // list on screen the tab keeps it and waits; with nothing on screen it
      // waits too, and says so if the read still fails once the machine is back.
      if (reads.failed(error)) return;
      notifyError("Could not read this project's issues", messageOf(error));
    }
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
    const issue = state.shown.find((candidate) => candidate.id === issueId);
    if (!issue || issue.status === status) return;
    const heldQuery = state.unscopedShown;
    const heldCatalogue = state.unscoped;
    const movedQuery = withMovedIssue(heldQuery, issueId, status);
    const movedCatalogue = withMovedIssue(heldCatalogue, issueId, status);
    state.focusIssue = issueId;
    await Promise.all([
      writeIssuesQueryRecord(state.deviceId, state.projectId, queryParams(), issuesRecord(movedQuery, state.columns)),
      writeIssuesRecord(state.deviceId, state.projectId, issuesRecord(movedCatalogue, state.columns)),
    ]);
    try {
      await state.callRpc("issues.update", moveParams(issueId, status));
    } catch (error) {
      if (state.disposed) return;
      state.focusIssue = issueId;
      await Promise.all([
        writeIssuesQueryRecord(state.deviceId, state.projectId, queryParams(), issuesRecord(heldQuery, state.columns)),
        writeIssuesRecord(state.deviceId, state.projectId, issuesRecord(heldCatalogue, state.columns)),
      ]);
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

  /** The issue was filed and the assignee was not. Said after the fact rather
   *  than in the form, because the filing SUCCEEDED — keeping the form up
   *  would invite a second one — and said persistently, because the reader
   *  believes an agent is working and none is. */
  const sayTheAssigneeWasDropped = (issue) =>
    notifyError(
      `Filed #${issue?.number ?? ""} — but nobody was assigned`,
      "The Build on this machine cannot assign an issue as it is filed, so it was created unassigned and nothing was started. Assign it from the issue page.",
    );

  /** The issue was filed and the files were not (#57). The same shape of
   *  silence and the same answer: the screenshot was usually the reason for
   *  filing, so "it is not there" is worth a sentence rather than a discovery. */
  const sayTheFilesWereDropped = (issue, count) =>
    notifyError(
      `Filed #${issue?.number ?? ""} — but ${count === 1 ? "the file" : `the ${count} files`} did not go with it`,
      "The Build on this machine cannot carry files on an issue, so it was filed without them. Nothing was lost on your side — attach them to a comment once this machine can take them.",
    );

  /**
   * A freshly filed issue, on screen before the read that confirms it.
   *
   * The list is keyed (core/trackerIssuesBody.js), so this is one row inserted
   * rather than a repaint — which is the whole point of filing in place: you
   * see the thing you just wrote appear in the list you wrote it against. The
   * refresh behind it replaces this record with the bridge's own.
   */
  async function showTheNewIssue(issue) {
    if (!issue || state.disposed) return;
    const held = kept([issue]);
    if (!held.length) return; // a workspace's tab may not be about this issue
    const catalogue = sortIssues([...state.unscoped.filter((one) => one.id !== issue.id), issue]);
    const query = sortIssues([...state.unscopedShown.filter((one) => one.id !== issue.id), issue]);
    state.focusIssue = issue.id;
    await Promise.all([
      writeIssuesRecord(state.deviceId, state.projectId, issuesRecord(catalogue, state.columns)),
      writeIssuesQueryRecord(state.deviceId, state.projectId, queryParams(), issuesRecord(query, state.columns)),
    ]);
  }

  /** Put the keyboard on a row by the issue it is about — by a scan rather
   *  than a selector, because an id is data and a selector is a language. */
  const focusRow = (issueId) => {
    for (const row of host.querySelectorAll(".issue-row")) {
      if (row.dataset.issue === issueId) {
        row.querySelector(".issue-row-open")?.focus();
        return true;
      }
    }
    return false;
  };

  const focusDashboardRow = (issueId) => {
    for (const row of host.querySelectorAll(".issue-dashboard-row")) {
      if (row.dataset.issue === issueId) {
        row.querySelector(".issue-dashboard-link")?.focus();
        return true;
      }
    }
    return false;
  };

  /**
   * Open the composer in place (#57).
   *
   * In the slot between the bar and the rows, not over them: an issue is filed
   * ABOUT the list it is filed into, and a dialog that covers the list takes
   * away the one thing you were looking at while you wrote. A second press on
   * New issue puts the focus back in the open one rather than opening another.
   */
  function fileIssue() {
    if (state.composer?.isOpen()) {
      chrome.composeSlot.querySelector(".issue-compose-title")?.focus();
      return;
    }
    state.composer = openIssueComposer(chrome.composeSlot, {
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
      attachable: carriesIssueAttachments(state.deviceId),
      callRpc: state.callRpc,
      onFiled: (answer, outcome) => {
        void showTheNewIssue(answer?.issue);
        void refresh();
        if (outcome?.assigneeWentNowhere) sayTheAssigneeWasDropped(answer?.issue);
        if (outcome?.attachmentsWentNowhere) sayTheFilesWereDropped(answer?.issue, outcome.attachmentCount);
      },
      onClosed: ({ filed }) => {
        state.composer = null;
        // Back where the reader was: on the row they just made, or on the
        // press that opened the form. Never nowhere, which is what closing a
        // focused subtree does if nobody says otherwise.
        if (!filed) host.querySelector("[data-issue-new]")?.focus();
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
      const assign = event.target.closest("[data-issue-assign]");
      if (!assign || !element.contains(assign)) return;
      event.preventDefault();
      openPicker(assign.dataset.issueAssign);
    };
  }

  /** A card is a row that can also be moved — dragged, or walked left and
   *  right from the keyboard. Neither is the accessible afterthought of the
   *  other, so both are wired on every card. */
  function wireCard(card) {
    wireRow(card);
    card.ondragstart = (event) => event.dataTransfer?.setData("text/plain", card.dataset.issue);
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
        const issueId = event.dataTransfer?.getData("text/plain");
        if (issueId) void moveIssue(issueId, column.dataset.columnDrop);
      };
    });
  }

  // ---- lifecycle -----------------------------------------------------------

  void uiRecord.ready.then(() => {
    if (state.disposed) return;
    paint();
    watchQuery();
    void paintUserSession();
    void paintFromCache().then(() => refresh());
  });
  // No cadence: nothing in this client polls. The tab hears that an issue of
  // this project moved and reads the list again, and the pass behind it
  // (core/cacheSync.js) is the whole of the safety net.
  const watcher = watchChanges({
    refresh: () => void refresh(),
    entity: state.projectId,
    deviceId: state.deviceId,
  // Named only where the bridge carries them (core/trackerPush.js): every
  // kind in one subscribe shares that call's fate, and a refused one takes
  // this device's other subscriptions with it.
    kinds: issuesPushKinds(state.deviceId),
    mode: "realtime",
  });

  return {
    /** The feed moved: the agents a picker would offer may have, so the next
     *  paint names them again. Nothing is re-read from the bridge. */
    feedMoved() {
      rescope();
      details.updateIssues(state.shown);
      void activity.updateFeed(state.feed(), state.projectKey);
      paint();
    },
    dispose() {
      state.disposed = true;
      uiRecord.dispose();
      watcher.dispose();
      wholeListWatcher?.();
      userSessionWatcher?.();
      details.dispose();
      activity.dispose();
      queryUnsubscribe?.();
      reads.dispose();
      chrome.dispose();
      state.picker?.close?.();
      state.composer?.close?.();
    },
  };
}
