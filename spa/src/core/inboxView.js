// The inbox rail's entries: the feed, the paint, the four things a row can do —
// open, Done, mute, and say why it failed — and the one disclosure at the end
// of the list, Recent.
//
// The rail has two faces. The inbox is the one list across every project; the
// projects face is that same list grouped by project, with a Recent fold per
// block, the block's head opening the project's checkout and offering the
// create surface, and a new-project control at the top. The rows are the same
// rows either way, and so is everything a row can do.
//
// WHICH rows appear and what they say is core/inbox.js and core/inboxProjects.js;
// this module is the wiring. Read state is the bridge's now (`entity.seen`), so
// opening an entry tells the daemon, and nothing about what has been read is
// kept on the device.

import { $, el } from "../dom.js";
import { App } from "../app.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { confirmAction } from "./confirm.js";
import {
  activeEntryKey,
  branchDeleteTooOld,
  branchDoneConfirm,
  byAnchor,
  captureEntries,
  dismissParamsOf,
  entryKeyOf,
  inboxEmptyHtml,
  inboxRowHtml,
  taskDoneConfirm,
  mergePendingRows,
  recentIsOpen,
  recentToggleHtml,
  watchedWorkspaceEntries,
  workspaceEntryKey,
  workspaceIsRecent,
} from "./inbox.js";
import { projectAgentEntries } from "./inboxProjectAgent.js";
import { NO_TASK_UNREAD } from "./taskUnread.js";
import { patchList } from "./patchList.js";
import { BRANCH_DONE_OPTION, branchFinishFailureSummary, branchFinishParams, branchFinishNotice } from "./branchFinish.js";
import { readBranchDelete } from "./branchDeleteSupport.js";
import { projectOptimistic, reconcileOptimistic, subscribeOptimistic } from "./optimistic.js";
import { patchFeedRow, removeFeedRow } from "./cachedRows.js";
import { notifyError } from "./notify.js";
import { patchElement } from "./domPatch.js";
import { goFromInbox } from "./inboxShell.js";
import { routeProjectKey } from "./deviceKey.js";
import { indexRowsByEntity, markSeen, noteSelfAction } from "./inboxSeen.js";
import { canAnswer, contextFor, deviceFeedView, onDeviceStateChanged, whenGreeted } from "./deviceContexts.js";
import { filterByDevice, onlyDeviceRows } from "./deviceFilter.js";
import { awayRefusal, creationCall, paintDeviceState, verbCall } from "./inboxDevices.js";
import { CAPTURE_CONTROLS, captureError, disposeCaptureRows, initCaptureRows, onCaptureKeydown, reroutePicker } from "./inboxCaptures.js";
import { projectRoute } from "./projectModel.js";
import { hideProject } from "./projectHide.js";
import {
  blockIsFolded,
  projectBlockHtml,
  projectHeadHtml,
  projectsUnreadCount,
  rowDeviceNames,
  workspaceProjectBlocks,
} from "./inboxProjects.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { openCreateWork } from "./createWork.js";
import { openProjectSettings } from "../sheets/projectSettings.js";
import { openNewRepo } from "../sheets/newRepo.js";
import { branchOptions, mergeCaptureRows } from "./compose.js";
import { pendingCaptureRows, subscribePendingCaptures } from "./composeView.js";
import "../styles/shell.css";
import { publishInboxAttentionCount } from "./inboxAttention.js";
import { messageOf } from "./text.js";
import { followWatchedTasks } from "./watchedTaskFollower.js";
import { tasksAddress } from "./trackerCache.js";
import { noteWritten } from "./taskReadOrder.js";
import { mergeCachedAtomically } from "./localCache.js";

let items = [];
let runs = [];
// The board's rows for lifecycle verbs in flight (board.list's `pending`): a
// checkout being cut is on the list while its git runs.
let pendingLifecycle = [];
let projects = [];
// The snapshot those arrays were taken from, kept whole for the one thing that
// is about a single machine: where a capture can be rerouted to.
let snapshot = null;
let workspaces = [];
let entries = [];
let view = "inbox"; // which face the rail is showing: "inbox" or "projects"
let openMenuKey = null;
// Whether each Recent is open, once the user has said — keyed by whose Recent
// it is: the inbox's, or one project block's. A scope nobody has spoken for
// lets its partition decide (it opens when the list above it is thin).
const recentOpen = new Map();
// What the user has said of each block's fold (project key → folded). A block
// they have said nothing about folds as the face decides. Remembered on this
// device.
let folds = new Map();
let foldRecord = null;
let recentRecord = null;
let menuRecord = null;
// Where a press leaves an open menu alone: a row's own action cluster, and a
// block head's ⋯ and the menu it opens.
const MENU_ZONES = ".inbox-actions, .inbox-project-head .inbox-more, .inbox-project-head > .inbox-menu";
const onOutsideMenu = (event) => {
  if (!event.target.closest(MENU_ZONES)) closeMenu();
};
// Where focus goes once the menu is painted: onto the first item of one that
// has just opened, or back onto the ⋯ of one the keyboard has just shut.
// `{ key, into }`, kept until the paint that carries it lands — a menu opens
// and shuts through its UI record, so the paint comes after the press.
let menuFocus = null;
const syncMenuDismissal = () => {
  document.removeEventListener("pointerdown", onOutsideMenu);
  if (openMenuKey !== null) document.addEventListener("pointerdown", onOutsideMenu);
};
const foldAddress = uiAddress({ view: "inbox", kind: "fold", sub: "projects" });
const recentAddress = uiAddress({ view: "inbox", kind: "fold", sub: "recent" });
const menuAddress = uiAddress({ view: "inbox", kind: "menu", sub: "entry" });
const writeFolds = () => foldRecord?.write({ entries: [...folds] });
// Each block as last painted, by project key: where its head opens, what it is
// called — which is what the create it offers is titled with — and the bare
// project id every RPC still wants.
let blocksPainted = new Map();
const errors = new Map(); // row key → the message its row is showing
const workspacesBeingFinished = new Set();
// The watched tasks that need the user (#125), followed through the cache
// while the rail is mounted.
let watchedTasks = null;
// Tasks the user stopped watching from their row, by row key: null while the
// unwatch is in flight, then the time the bridge stamped it. A list the sync
// layer asked for before the unwatch landed still says watched and writes
// that over the row's optimistic move, so the row stays away until the cached
// list has moved past the unwatch — a watch made after it brings it back.
const unwatched = new Map();
const watchedTaskRows = () => (watchedTasks?.entries() || []).filter((entry) => !heldAway(entry));

function heldAway(entry) {
  if (!unwatched.has(entry.key)) return false;
  const stampMs = unwatched.get(entry.key);
  if (stampMs == null || entry.anchorMs == null || entry.anchorMs <= stampMs) return true;
  unwatched.delete(entry.key);
  return false;
}


/** A box in the list has the caret. The reconciler keeps a row that is still
 *  there, and the box in it with the words and the caret — so a repaint no
 *  longer takes what is being typed. This is the outer guard on top of that: a
 *  tick nobody asked for leaves a half-named destination alone, whatever else
 *  the paint would have decided. The user's own actions still repaint. */
function typingInList() {
  const active = document.activeElement;
  return Boolean(active && active.tagName === "INPUT" && active.closest("#inbox-list"));
}

/** The repaint the feed asks for, which is the one that must wait. */
function drawFromFeed() {
  if (typingInList()) {
    publishAttentionCount();
    return;
  }
  draw();
}

/** The top badge (#183): the sum of the projects face's head badges, over the
 *  rows either face paints. */
function publishAttentionCount(rows = railRows()) {
  publishInboxAttentionCount(projectsUnreadCount(rows, projects));
}

/** The workspace rows and each project's own agent row (#103), together in
 *  the inbox's anchor order. On the projects face the project agent's row is
 *  its block's head rather than a row (core/inboxProjects.js). */
function workRows(rows) {
  // A watched task's unread counts like an agent's (#104): on the workspace
  // row whose agent holds it, and on the project agent's row otherwise.
  const taskUnread = watchedTasks?.taskUnread() || NO_TASK_UNREAD;
  const workspaceRows = watchedWorkspaceEntries(workspaces, projects, rows, runs, taskUnread);
  return [
    ...workspaceRows,
    ...projectAgentEntries(projects, rows, runs, taskUnread.unheldBy(workspaceRows)),
  ].sort(byAnchor);
}

/** The one name a row has, which is what the reconciler matches rows by. */
const keyOf = (entry) => entry.key;

export const INBOX_SCOPE = "inbox";

// The read cursor has its own module (core/inboxSeen.js); its callers still
// find it here.
export { markSeen, noteSelfAction };

// The captures this client is holding or watching stand beside the daemon's
// own rows; the daemon's copy wins wherever both name the same capture. They
// are narrowed by the picker on the way in — the snapshot's own rows were
// narrowed as it arrived, and a row is a row whoever is holding it.
const mergedItems = () =>
  mergeCaptureRows(mergePendingRows(items, pendingLifecycle), onlyDeviceRows(pendingCaptureRows(), App.deviceFilter));

/** Show one of the rail's two faces. The shell calls this with what the user
 *  chose (and remembered); the list repaints as that face. */
export function setInboxView(next) {
  if (next === view) return;
  view = next;
  openMenuKey = null;
  if (menuRecord) void menuRecord.write({ key: null });
  draw();
}

/** Every row the rail paints, on either face. The captures first: they are
 *  the account's unfinished business and belong to no project, so they stand
 *  above the workspace rows on the flat face and above the blocks on the
 *  other. The watched tasks asking for the user come next, and sit in their
 *  project's block on the projects face. */
function railRows() {
  const rows = projectOptimistic(INBOX_SCOPE, mergedItems(), { keyOf: entryKeyOf });
  return [...captureEntries(rows), ...watchedTaskRows(), ...workRows(rows)];
}

function draw() {
  watchedTasks?.follow(projects);
  const painted = railRows();
  publishAttentionCount(painted);
  const list = $("#inbox-list");
  if (!list) return;
  const shown = withDeviceNames(painted);
  list.onclick = onListClick;
  list.onkeydown = onListKeydown;
  // A different face is a different list: the one is emptied for the other,
  // and every paint after that reconciles in place.
  if (list.dataset.view !== view) {
    list.dataset.view = view;
    list.replaceChildren();
  }
  const scroll = list.scrollTop;
  if (view === "projects") drawProjects(list, shown);
  else drawWorkspaceList(list, shown);
  list.scrollTop = scroll;
  paintErrors(list);
  paintDeviceState(list, { entryFor: entryOf, blockFor: blockOf });
  placeMenuFocus(list);
}

/** Which machine each row is to say it is on: only where two machines use the
 *  same project name, decided once for the whole list so the row painters print
 *  what they are given. A rail showing one machine's work names no machine at
 *  all, which is every account with one device and every filtered rail. */
function withDeviceNames(rows) {
  const names = rowDeviceNames({ items: rows, projects, devices: App.devices });
  return rows.map((row) => ({ ...row, deviceName: names.get(row.projectKey) || null }));
}

/** The inbox face's flat durable workspace list. */
function drawWorkspaceList(list, shown) {
  entries = shown;
  const ui = rowUi(true);
  paintEmpty(list, entries.length === 0, inboxEmptyHtml, ".inbox-clear");
  list.querySelector(":scope > .inbox-unsorted")?.remove();
  list.querySelector(":scope > .inbox-projects")?.remove();
  const active = shown.filter((entry) => !workspaceIsRecent(entry));
  const recent = shown.filter((entry) => workspaceIsRecent(entry));
  patchList(list, active, { keyOf, render: (entry) => inboxRowHtml(entry, ui) });
  paintRecent(list, recent, ui, "inbox");
}

/** What every row is painted with. `showProject` is whether a row names its
 *  own project — under a project block it has already been told. */
function rowUi(showProject) {
  const picker = reroutePicker();
  // A reroute goes to the machine holding the capture, so the destinations it
  // offers are that machine's — its projects, and the branches they have.
  const destinations = deviceFeedView(snapshot, entryOf(picker.rerouteKey)?.deviceId);
  const activeKey = activeEntryKey(App.route, entries);
  return {
    activeKey,
    openMenuKey,
    projects: destinations.projects,
    ...picker,
    // The branches that project already has, off the same feed rows the
    // compose panel offers: one source for "which branches are there".
    rerouteBranches: branchOptions(destinations.items, picker.rerouteBranchProject),
    showProject,
    folded: new Set(),
    // The block holding the branch or task the route stands on. A capture's
    // route names no project; the row it stands on does.
    activeProjectId: activeProjectKey(activeKey),
    finishingWorkspaces: workspacesBeingFinished,
  };
}

/** The block the route stands in, named the way every block is named: the row
 *  the route opens says so, and a route that matches no row still names its own
 *  machine and project (core/deviceKey.js). */
function activeProjectKey(activeKey) {
  const standing = entries.find((entry) => entry.key === activeKey);
  if (standing) return standing.projectKey || null;
  return routeProjectKey(App.route);
}

/** The machines that cannot be asked anything right now, by id — the account's
 *  own list is the set of machines there are, and a machine's context says
 *  whether it can answer (core/deviceContexts.js). A project whose machine is
 *  not on the list at all is offline by the same rule: nothing can answer for
 *  it, and deviceTags reads that off the list it is already handed. */
const offlineDeviceIds = () =>
  new Set(App.devices.map((device) => device.id).filter((id) => id && !canAnswer(contextFor(id))));

/** The projects face: any workspaces whose project nothing lists, then one
 *  block per project — every machine's, each head naming its machine where two
 *  machines use that project name. */
function drawProjects(list, shown) {
  const { unsorted, blocks, recentBlocks } = workspaceProjectBlocks(shown, projects, App.devices, offlineDeviceIds());
  const allBlocks = [...blocks, ...recentBlocks];
  entries = [...unsorted, ...allBlocks.flatMap((block) => [...block.entries, ...block.recent])];
  blocksPainted = new Map(allBlocks.map((block) => [block.projectKey, block]));
  const folded = new Set(allBlocks.filter((block) => blockIsFolded(block, folds)).map((block) => block.projectKey));
  const ui = { ...rowUi(false), folded };
  const frame = projectsFrame(list);
  patchList(frame.unsorted, unsorted, { keyOf, render: (entry) => inboxRowHtml(entry, ui) });
  paintBlocks(frame.blocks, blocks, ui);
  paintRecentProjects(list, recentBlocks, ui);
}

/** A project's whole block, including its workspaces, moves into this
 * disclosure after a day without a message. The block remains keyed. */
function paintRecentProjects(list, blocks, ui) {
  let section = list.querySelector(":scope > .inbox-recent");
  if (!blocks.length) {
    section?.remove();
    return;
  }
  if (!section) {
    section = el('<div class="inbox-recent"><div class="inbox-projects"></div></div>');
    section.prepend(el(recentToggleHtml(blocks, false, "projects")));
  }
  if (list.lastElementChild !== section) list.appendChild(section);
  const open = recentIsOpen(recentOpen.get("projects"));
  patchElement(section.querySelector("[data-recent-toggle]"), el(recentToggleHtml(blocks, open, "projects")));
  paintBlocks(section.querySelector(":scope > .inbox-projects"), open ? blocks : [], ui);
}

/** The projects face's frame, built once: the new-project control, the loose
 *  rows' container, and the blocks'. */
function projectsFrame(list) {
  let unsorted = list.querySelector(":scope > .inbox-unsorted");
  if (!unsorted) {
    unsorted = el('<div class="inbox-unsorted"></div>');
    list.append(unsorted, el('<div class="inbox-projects"></div>'));
  }
  return { unsorted, blocks: list.querySelector(":scope > .inbox-projects") };
}

/** The blocks, reconciled by project: a block that is still there keeps its
 *  element — and so its rows keep theirs, since each block's rows are its own
 *  keyed list — and only its head is patched. Blocks are placed back to front,
 *  each in front of the one that follows it, so a project that moved costs
 *  one move and the rest stay put. */
function paintBlocks(host, blocks, ui) {
  const wanted = new Set(blocks.map((block) => block.key));
  const standing = new Map();
  for (const child of [...host.children]) {
    if (wanted.has(child.dataset.key)) standing.set(child.dataset.key, child);
    else child.remove();
  }
  let anchor = null;
  for (let index = blocks.length - 1; index >= 0; index -= 1) {
    const block = blocks[index];
    let element = standing.get(block.key);
    if (!element) {
      element = el(projectBlockHtml(block, ui));
      host.insertBefore(element, anchor);
    } else {
      element.classList.toggle("inbox-flat", block.flat);
      element.classList.toggle("inbox-folded", ui.folded.has(block.projectKey));
      element.classList.toggle("active", ui.activeProjectId === block.projectKey);
      patchElement(element.querySelector(":scope > .inbox-project-head"), el(projectHeadHtml(block, ui)));
      if (element.nextSibling !== anchor) host.insertBefore(element, anchor);
    }
    const rows = element.querySelector(":scope > .inbox-project-rows");
    if (block.flat) {
      // Nothing live: the quiet rows stand straight under the head, and the
      // block's own chevron is their fold — no Recent disclosure of their own.
      element.querySelector(":scope > .inbox-recent")?.remove();
      patchList(rows, block.recent, { keyOf, render: (entry) => inboxRowHtml(entry, { ...ui, quiet: true }) });
    } else {
      patchList(rows, block.entries, { keyOf, render: (entry) => inboxRowHtml(entry, ui) });
      paintRecent(element, block.recent, ui, block.projectKey);
    }
    anchor = element;
  }
}

/** The line a container shows in place of rows when it is holding nothing. It
 *  is chrome, not a row, and it goes the moment there is a row — a row is
 *  placed after the last row, so nothing else may be sitting down there. */
function paintEmpty(container, empty, html, selector) {
  const standing = container.querySelector(selector);
  if (empty === Boolean(standing)) return;
  if (standing) standing.remove();
  else container.appendChild(el(html()));
}

/** Recent: the disclosure at the end of a list, and behind it the rows that
 *  have gone quiet. It is its own container, so the quiet rows are its keyed
 *  children and each list reconciles only its own. `scope` names whose Recent
 *  it is, which is what the user's open-or-shut is remembered under. */
function paintRecent(host, recent, ui, scope) {
  if (!recent.length) {
    host.querySelector(":scope > .inbox-recent")?.remove();
    return;
  }
  let section = host.querySelector(":scope > .inbox-recent");
  if (!section) {
    section = document.createElement("div");
    section.className = "inbox-recent";
    section.append(el(recentToggleHtml(recent, false, scope)));
  }
  // Recent follows the list proper. A row that arrives while nothing was keyed
  // above it lands after the section, so the section is put back at the end
  // whenever a paint has left something below it.
  if (host.lastElementChild !== section) host.appendChild(section);
  const open = recentIsOpen(recentOpen.get(scope));
  patchElement(section.querySelector("[data-recent-toggle]"), el(recentToggleHtml(recent, open, scope)));
  // Recent's rows are quiet rows: one line each, no state dot.
  patchList(section, open ? recent : [], { keyOf, render: (entry) => inboxRowHtml(entry, { ...ui, quiet: true }) });
}

/** A row key is whatever the daemon minted (a worktree's is derived from a
 *  path), so rows are matched by reading their key back, never by building a
 *  selector out of it. */
function paintErrors(list) {
  list.querySelectorAll(".inbox-entry").forEach((row) => {
    const message = errors.get(row.dataset.key) || captureError(row.dataset.capture);
    const slot = message && row.querySelector("[data-done-error], [data-capture-error]");
    if (!slot) return;
    slot.textContent = message;
    slot.hidden = false;
  });
}

/** The two things the wiring keeps of what it last painted, looked up the same
 *  way: a row by its key, and a block by its project key. Nothing is ever found
 *  by a selector built out of an id the daemon minted. */
const entryOf = (key) => entries.find((entry) => entry.key === key) || null;
const blockOf = (projectKey) => blocksPainted.get(projectKey) || null;

function closeMenu() {
  if (openMenuKey === null) return;
  if (menuRecord) void menuRecord.write({ key: null });
  else {
    openMenuKey = null;
    syncMenuDismissal();
    draw();
  }
}

/** One control per attribute a row paints, in the order a press is read in:
 *  the innermost control wins. Each is handed the element that was pressed. */
const ROW_CONTROLS = [
  ["data-workspace-done", (control) => finishWorkspace(entryOf(control.dataset.workspaceDone))],
  ["data-done", (control) => finishRow(control.dataset.done)],
  ["data-mute", (control) => toggleMute(entryOf(control.dataset.mute))],
  ["data-unwatch", (control) => unwatchTask(entryOf(control.dataset.unwatch))],
  ["data-dismiss", (control) => dismissEntry(entryOf(control.dataset.dismiss))],
  ["data-menu", (control) => openMenu(control.dataset.menu)],
  // Recent is one disclosure, and pressing it is the user saying so — from then
  // on the section stays as they left it, whatever the list above it does.
  ["data-recent-toggle", (control) => toggleRecent(control)],
];

/** Done is the one row verb that comes off the menu, so the menu shuts with it. */
function finishRow(key) {
  closeMenu();
  finishEntry(entryOf(key));
}

function toggleRecent(control) {
  recentOpen.set(control.dataset.recentToggle, control.getAttribute("aria-expanded") !== "true");
  if (recentRecord) void recentRecord.write({ entries: [...recentOpen] });
  else draw();
}

/** The press answered off a table of controls: true when it was one of them. */
function pressed(controls, target) {
  for (const [attribute, act] of controls) {
    const control = target.closest(`[${attribute}]`);
    if (control) {
      act(control);
      return true;
    }
  }
  return false;
}

/** One control per attribute the head paints, in the order a press is read in:
 *  the innermost control wins, so the fold and the + are asked for before the
 *  name they sit beside. Each is handed the element that was pressed. */
const BLOCK_CONTROLS = [
  ["data-project-fold", (control) => toggleFold(control.dataset.projectFold)],
  ["data-project-open", (control) => openBlockHead(control.dataset.projectOpen)],
  ["data-project-settings", (control) => settingsForBlock(control.dataset.projectSettings)],
  ["data-project-create", (control) => createInBlock(control.dataset.projectCreate)],
  ["data-project-hide", (control) => hideBlock(control.dataset.projectHide)],
  ["data-new-project", () => openNewProject()],
];

/** Every control the list holds, innermost first: a row's own verbs, then what
 *  a capture row can do to its route, then a block's head. */
const LIST_CONTROLS = [...ROW_CONTROLS, ...CAPTURE_CONTROLS, ...BLOCK_CONTROLS];

/// Every control in the list, answered in one place.
///
/// A row's element survives the paints, but a control inside it does not have
/// to — Done appears the moment the work can be finished — so nothing is wired
/// to a row or to a button. The list itself listens, and reads off the DOM which
/// row was spoken for.
function onListClick(event) {
  const { target } = event;
  if (pressed(LIST_CONTROLS, target)) return;
  // The row's own controls answer for themselves; everything else on it opens.
  const row = target.closest(".inbox-entry");
  if (row && !target.closest(".inbox-actions")) openEntry(entryOf(row.dataset.key));
}

/** The row's menu, one step behind the row: it opens, and the next press
 *  anywhere outside it shuts it again. Opening puts focus on its first item,
 *  where the keyboard walks it from. */
function openMenu(key) {
  const next = openMenuKey === key ? null : key;
  menuFocus = next === null ? null : { key: next, into: true };
  if (menuRecord) void menuRecord.write({ key: next });
  else {
    openMenuKey = next;
    syncMenuDismissal();
    draw();
  }
}

/** The ⋯ a menu is opened by, and the menu it opens: a row's sits beside it in
 *  the row's actions, a block's hangs off the block's head. */
const moreButtonOf = (list, key) => [...list.querySelectorAll("[data-menu]")].find((button) => button.dataset.menu === key);
const menuOf = (more) => more?.closest(".inbox-actions, .inbox-project-head")?.querySelector(".inbox-menu") || null;
const menuItems = (menu) => [...menu.querySelectorAll(".mi")];

/** Once the paint carrying the menu's new state has landed, focus goes where
 *  `menuFocus` said: into the open menu, or back onto its ⋯. */
function placeMenuFocus(list) {
  if (!menuFocus || menuFocus.into !== (openMenuKey === menuFocus.key)) return;
  const more = moreButtonOf(list, menuFocus.key);
  const target = menuFocus.into ? menuOf(more)?.querySelector(".mi") : more;
  menuFocus = null;
  target?.focus({ preventScroll: true });
}

/** Shut the open menu from the keyboard, with focus back on its ⋯. */
function shutMenuToOpener() {
  menuFocus = { key: openMenuKey, into: false };
  closeMenu();
}

/** Move focus to the item `step` along from the focused one, wrapping; `step`
 *  null is Home, and -Infinity End. */
function walkMenu(menu, step) {
  const items = menuItems(menu);
  if (!items.length) return;
  const at = items.indexOf(document.activeElement);
  const next = step === null ? 0 : step === -Infinity ? items.length - 1 : at + step;
  items[((next % items.length) + items.length) % items.length].focus({ preventScroll: true });
}

/** The keys an open rail menu answers, from inside it: the arrows walk its
 *  items and wrap, Home and End jump, Escape shuts it with focus back on its
 *  ⋯, and Tab shuts it as focus leaves. Enter and Space are the focused item's
 *  own, since each is a button. */
const MENU_KEYS = {
  ArrowDown: (menu) => walkMenu(menu, 1),
  ArrowUp: (menu) => walkMenu(menu, -1),
  Home: (menu) => walkMenu(menu, null),
  End: (menu) => walkMenu(menu, -Infinity),
  Escape: () => shutMenuToOpener(),
};

/** A key pressed in an open menu, or Escape on the ⋯ of one: true when the
 *  menu answered it. */
function menuKey(event) {
  if (openMenuKey === null) return false;
  const menu = event.target.closest(".inbox-menu");
  if (!menu) {
    if (event.key !== "Escape" || !event.target.closest("[data-menu]")) return false;
    shutMenuToOpener();
    return true;
  }
  if (event.key === "Tab") closeMenu();
  const act = MENU_KEYS[event.key];
  if (act) act(menu);
  return Boolean(act);
}

function onListKeydown(event) {
  if (!menuKey(event)) return onCaptureKeydown(event);
  event.preventDefault();
  event.stopPropagation();
}

// ---- project blocks -----------------------------------------------------------
//
// What a block's head can do: fold, open the project's own page, open its
// settings, create — a workspace, on the one create surface, scoped to the
// block's project — and, behind its ⋯, hide it. And the one control above every
// block: a new project. Which press is which is the table BLOCK_CONTROLS, up
// with the other control tables.

/** The block's name opens the project's own page — its workspaces, and the
 *  agent you talk to about the project. Every project has one, so every head
 *  opens. */
function openBlockHead(projectKey) {
  const block = blockOf(projectKey);
  if (block) goFromInbox(block.route);
}

/** What each verb the settings sheet sends was for, in the reader's words — the
 *  sentence a press on it says when the block's machine cannot take it. A verb this
 *  does not name is still a change to the project's settings. */
const SETTINGS_DOING = {
  "project.list": "open this project's settings",
  "project.set_remote": "save this project's remote",
  "project.set_isolation": "change this project's isolation",
  "project.add_source": "add a folder to this project",
  "project.remove_source": "remove this folder from this project",
  "project.delete": "delete this project",
  "settings.get": "list this machine's folders",
  "fs.list": "list this machine's folders",
  "fs.mkdir": "make a folder on this machine",
};
const settingsDoing = (method) => SETTINGS_DOING[method] || "change this project's settings";

/** Settings open on what the cache holds for the project, whether or not its
 *  machine is answering; each change the sheet sends goes through the block's
 *  own call, which refuses one the machine cannot take in a sentence saying
 *  so. */
function settingsForBlock(projectKey) {
  const block = blockOf(projectKey);
  if (!block) return;
  openProjectSettings(block.id, {
    callRpc: verbCall(block, settingsDoing),
    deviceId: block.deviceId,
    onDeleted: async () => {
      if (routeProjectKey(App.route) === block.projectKey) goFromInbox({ name: "inbox" });
      await refreshFeed(block.deviceId);
    },
  });
}

/** The + opens the create surface on this block's project, with the block
 *  unfolded so the new row has somewhere visible to land. The create surface
 *  talks to one bridge — the machine this block is on — which knows its
 *  projects by the bare id it minted. */
function createInBlock(projectKey) {
  const block = blockOf(projectKey);
  expandFold(projectKey);
  closeMenu();
  if (!block) return;
  openCreateWork({
    projectId: block.id,
    deviceId: block.deviceId,
    projectName: block.name,
    navigate: goFromInbox,
  });
}

/** Put a block away: it goes from the rail, and everything cached under it on
 *  its own machine goes with it (core/projectHide.js). Every block's menu
 *  offers it, and the fold the user had set for it goes too — a fold is about a
 *  block that is there, and this one is not coming back the same way. The rail
 *  repaints off the feed the drop delivers. */
function hideBlock(projectKey) {
  const block = blockOf(projectKey);
  closeMenu();
  if (!block) return;
  folds.delete(projectKey);
  void writeFolds();
  void hideProject({ deviceId: block.deviceId, projectKey });
}

/** The one control above every block, and the rail head's own: a project the
 * account does not have yet. The sheet honors the rail's device filter or asks
 * explicitly when the rail is showing every device. */
export function openNewProject() {
  openNewRepo((project, target) => {
    // A project is a template, so a fresh one opens the rail standing in it
    // rather than its own checkout (core/projectModel.js) — the block is where
    // the first workspace is made.
    //
    // The machine that made it is the machine it is on: the answer to a fresh
    // project.create is not a feed row and carries no device of its own, and a
    // device-less route is resolved by asking every machine — which would hand
    // the reader another machine's project of the same number.
    goFromInbox(projectRoute({ ...project, deviceId: target.id }));
    refreshFeed(target.id);
  }, {
    devices: App.devices,
    defaultDeviceId: App.deviceFilter,
    callRpcFor: creationCall,
  });
}

/** A fold is the user's, and it holds: across the feed, and across reloads. */
function toggleFold(projectKey) {
  const block = blockOf(projectKey);
  if (!block) return;
  folds.set(projectKey, !blockIsFolded(block, folds));
  if (foldRecord) void writeFolds();
  else draw();
}

/** Creating a branch gives the new row somewhere visible to land. */
function expandFold(projectKey) {
  const block = blockOf(projectKey);
  if (!block || !blockIsFolded(block, folds)) return;
  folds.set(projectKey, false);
  if (foldRecord) void writeFolds();
  else draw();
}

/** Opening an entry reads it — every agent on it — and goes where it lives. */
function openEntry(entry) {
  if (!entry) return;
  markSeen(entry.entityId).then(refreshFeed);
  if (entry.route) goFromInbox(entry.route);
}

async function toggleMute(entry) {
  if (!entry) return;
  const muted = !entry.muted;
  closeMenu();
  await optimisticVerb(entry, {
    write: () => patchFeedRow(entry.deviceId, entry, { muted }),
    call: () => verbCall(entry, `${muted ? "mute" : "unmute"} this item`)("entity.mute", { entity_id: entry.entityId, muted }),
    failureSummary: `Couldn't ${muted ? "mute" : "unmute"} ${entry.branch || "this item"}`,
  });
}

/** Stop watching a watched task's row (#125) — its Mute. The cached list
 *  says so at once, which takes the row away and says the same on the Tasks
 *  tab; the push that follows the unwatch confirms it, and a refusal puts the
 *  watch back. Until the list agrees the row is held away (`unwatched`). */
async function unwatchTask(entry) {
  if (!entry) return;
  closeMenu();
  await optimisticVerb(entry, {
    write: async () => {
      unwatched.set(entry.key, null);
      const undo = await markCachedWatch(entry, false);
      return () => {
        unwatched.delete(entry.key);
        return undo();
      };
    },
    call: async () => {
      const answer = await verbCall(entry, "stop watching this task")("tasks.unwatch", { task_id: entry.taskId });
      unwatched.set(entry.key, Date.parse(answer?.task?.updated_at || "") || entry.anchorMs);
    },
    failureSummary: `Couldn't stop watching ${entry.name}`,
  });
}

/** Set one task's watch in its project's cached list; answers the undo.
 *  Each is a list written here, so each takes its number from the read count
 *  (core/taskReadOrder.js) and the list carries it: newer than every read
 *  asked before it, the Tasks tab's own answers included, and a page of an
 *  older pull does not put the old watch back (#129). */
async function markCachedWatch(entry, watched) {
  const address = tasksAddress(entry.deviceId, entry.projectId);
  const setWatched = async (value) => {
    const writtenAs = await noteWritten([address], [entry.taskId]);
    return mergeCachedAtomically(address, (held) => held && {
      ...held,
      tasks: (held.tasks || []).map((task) => (task.id === entry.taskId ? { ...task, watched: value } : task)),
      read_order: writtenAs,
    });
  };
  await setWatched(watched);
  return () => setWatched(!watched);
}

/** Move the row to Recent until a new user or agent message. Nothing is
 *  destroyed, nothing is silenced: for a row with a conversation the daemon
 *  remembers how far every agent conversation had got, and the next message
 *  past any of those markers brings the row back by itself. The dismiss names
 *  the row the way
 *  dismissParamsOf says: an entity by its id, an entity-less row by what it is.
 *
 *  The row moves optimistically on the tap and returns to Inbox if the daemon
 *  refuses. */
async function dismissEntry(entry) {
  const params = entry && dismissParamsOf(entry);
  if (!params) return;
  closeMenu();
  await optimisticVerb(entry, {
    write: () => patchFeedRow(entry.deviceId, entry, { dismissed: true }),
    call: async () => {
      // Clearing an unread row also acknowledges its unread notification. The
      // dismissal controls placement, but preserving read state avoids an
      // obsolete badge if the entry later returns.
      const call = verbCall(entry, "clear this item");
      if (entry.state === "unread" && entry.entityId) {
        await call("entity.seen", { entity_id: entry.entityId });
      }
      await call("entity.dismiss", params);
    },
    failureSummary: `Couldn't clear ${entry.branch || "this item"}`,
  });
}

/** The RPC behind Done. On a branch it DELETES: the branch (where the bridge
 *  deletes it, `target.deletesBranch`), its checkout and its records go,
 *  which is what Done on a branch means. On a task it archives. Neither is
 *  refused for the state of the work — what the destruction costs came down
 *  with the row and was confirmed through. */
export async function finishWorkItem(target, optionId = BRANCH_DONE_OPTION) {
  const call = verbCall(target, finishDoing(target));
  if (target.kind === "task") await call("plan.archive", { plan_id: target.taskId });
  else sayBranchFinishNotice(target, await sendBranchFinish(target, call, optionId));
  // Done ends the work, and an ending is an attention event. The user did this
  // here, so this entry is already read. The task an unmerged branch leaves
  // behind is NOT: it comes back to the inbox asking for somebody, and the
  // event naming the branch it lost is the whole point of it coming back.
  await noteSelfAction(target.entityId, target.taskEnded ? target.taskId : null);
}

/** The workspace is gone; say why a requested branch deletion was refused or
 *  why a deleted branch could not be restored. */
function sayBranchFinishNotice(target, answer) {
  const notice = branchFinishNotice(target.branch, answer);
  if (notice) notifyError(notice.summary, notice.detail);
}

/** What Done does, in the words a refusal names it by. */
function finishDoing(target) {
  if (target.kind === "task") return "archive this task";
  return target.deletesBranch ? "delete this branch" : "remove this checkout";
}

/** Send Done's `branch.finish`. A deletion goes out only on the verdict of
 *  the greeting the machine's current session is on (#87): the cache said the
 *  bridge deletes when the confirmation was drawn, but a session adopted since
 *  may be an older bridge whose hello has not answered, and it would drop the
 *  word. So the send waits on that greeting, and a bridge that keeps the
 *  branch is refused in words rather than sent a word it would drop. */
async function sendBranchFinish(target, call, optionId) {
  const send = (deletesBranch) =>
    call("branch.finish", branchFinishParams(optionId, { projectId: target.projectId, branch: target.branch, deletesBranch }));
  if (!target.deletesBranch) return send(false);
  const context = contextFor(target.deviceId);
  const request = context && (await whenGreeted(context, () => {
    if (context.adapter?.capabilities?.branches?.finishDelete !== true) throw new Error(branchDeleteTooOld(target.deviceName));
    return send(true);
  }));
  if (!request) throw new Error(awayRefusal(finishDoing(target), context));
  return request.sent;
}

async function finishEntry(entry) {
  if (!entry) return;
  // What Done on this machine does to the branch, from the cache (#87).
  const target = entry.kind === "task" ? entry : { ...entry, deletesBranch: await readBranchDelete(entry.deviceId) };
  const confirmation = entry.kind === "task" ? taskDoneConfirm(entry) : branchDoneConfirm(target);
  if (!(await confirmAction(confirmation))) return;
  // Confirmation is the decisive moment: the row goes now, and the git work
  // (and the push that confirms it) carries on behind it.
  await optimisticVerb(entry, {
    write: () => removeFeedRow(entry.deviceId, entry),
    call: () => finishWorkItem(target),
    failureSummary: branchFinishFailureSummary(entry.branch),
  });
}

/** Put a finished workspace away in one tap. Done removes it: the bridge
 * re-measures the work at execution time, stops every agent and terminal
 * standing in it, hands its checkouts back and walks the root away. No
 * confirmation, because the bridge does not offer Done until the work is
 * already in a remote. */
async function finishWorkspace(entry) {
  if (!entry || entry.kind !== "workspace" || !entry.canFinish || workspacesBeingFinished.has(entry.key)) return;
  workspacesBeingFinished.add(entry.key);
  errors.delete(entry.key);
  draw();
  try {
    await verbCall(entry, "archive this workspace")("workspace.finish", { workspace_id: entry.workspaceId });
    workspaces = workspaces.filter((workspace) => workspace.workspaceKey !== entry.workspaceKey);
    leaveFinishedWorkspace(entry);
  } catch (error) {
    errors.set(entry.key, messageOf(error));
  } finally {
    workspacesBeingFinished.delete(entry.key);
    draw();
  }
  await refreshFeed();
}

/** A surface standing in a workspace that is gone has nothing behind it, so
 *  the page leaves for the project the workspace belonged to. A reader
 *  somewhere else is left where they are. */
function leaveFinishedWorkspace(entry) {
  const route = App.route;
  if (route?.name !== "workspace") return;
  if (route.workspaceId !== entry.workspaceId || route.deviceId !== entry.deviceId) return;
  goFromInbox(projectRoute({ id: entry.projectId, deviceId: entry.deviceId }));
}

function showRowError(key, error) {
  errors.set(key, messageOf(error));
  draw();
}

// The rows a verb is in flight for, so a second tap on one is not a second
// verb. What `isPending` answered while the rail kept its moves in memory.
const acting = new Set();

/**
 * One press that moves a row: the move is written into the cache, the verb is
 * sent, and the push that follows confirms it.
 *
 * The cache is the only thing a view reads, so writing there is the whole of
 * showing the move — every surface holding the row hears it, and the move
 * outlives a remount and a reload. A bridge that refuses puts back exactly
 * what was there and says so on the row.
 */
async function optimisticVerb(entry, { write, call, failureSummary }) {
  if (acting.has(entry.key)) return;
  acting.add(entry.key);
  errors.delete(entry.key);
  const undo = await write();
  try {
    await call();
  } catch (error) {
    await undo();
    showRowError(entry.key, error);
    notifyError(failureSummary, messageOf(error));
  } finally {
    acting.delete(entry.key);
  }
}

let mounted = false;
let stopSubscriptions = [];

/** Let a rail that is leaving release its callbacks, including captures that
 *  can settle after its DOM has gone. The app normally keeps the rail mounted;
 *  a test or an account teardown may remove it. */
export function unmountInboxList() {
  if (!mounted) return;
  mounted = false;
  stopSubscriptions.forEach((stop) => stop());
  stopSubscriptions = [];
  foldRecord?.dispose();
  recentRecord?.dispose();
  menuRecord?.dispose();
  watchedTasks?.dispose();
  watchedTasks = null;
  document.removeEventListener("pointerdown", onOutsideMenu);
  foldRecord = null;
  recentRecord = null;
  menuRecord = null;
  disposeCaptureRows();
}

/** Mount once. Re-entrant: a reconnect calls this again and it just repaints. */
export function mountInboxList() {
  if (mounted) {
    draw();
    return;
  }
  mounted = true;
  folds = new Map();
  recentOpen.clear();
  openMenuKey = null;
  foldRecord = watchUiState(foldAddress, (saved) => {
    folds = new Map(saved?.entries || []);
    draw();
  });
  recentRecord = watchUiState(recentAddress, (saved) => {
    recentOpen.clear();
    for (const [key, value] of saved?.entries || []) recentOpen.set(key, value);
    draw();
  });
  menuRecord = watchUiState(menuAddress, (saved) => {
    openMenuKey = saved?.key || null;
    syncMenuDismissal();
    draw();
  });
  initCaptureRows({ onChange: draw, entryOf });
  watchedTasks = followWatchedTasks({ onChange: drawFromFeed });
  // A machine going or coming back changes no row, so the feed never says it:
  // the rail hears it from the registry and repaints, greying what the lost
  // device holds. Its verbs stay as they were; each refuses at the press.
  stopSubscriptions = [
    onDeviceStateChanged(draw),
    subscribePendingCaptures(drawFromFeed),
    subscribeOptimistic(INBOX_SCOPE, draw),
    subscribeFeed((next) => {
      // Which machines the rail lists is the picker's, and it is answered once,
      // here: everything below paints whatever this snapshot holds.
      snapshot = filterByDevice(next, App.deviceFilter);
      items = snapshot.items || [];
      runs = snapshot.runs || [];
      pendingLifecycle = snapshot.pending || [];
      projects = snapshot.projects || [];
      workspaces = snapshot.workspaces || [];
      const live = new Set([
        ...items.map(entryKeyOf),
        ...workspaces.map(workspaceEntryKey),
        ...watchedTaskRows().map(keyOf),
      ]);
      for (const key of errors.keys()) if (!live.has(key)) errors.delete(key);
      const merged = mergedItems();
      // Every verb that names only an entity — a read report, a self-action —
      // finds its row, and so its device, through this.
      indexRowsByEntity(merged);
      reconcileOptimistic(INBOX_SCOPE, merged, { keyOf: entryKeyOf });
      drawFromFeed();
    }),
  ];
}

/** Repaint so the entry the route stands on is the marked one. */
export function inboxListRouteChanged() {
  if (mounted) draw();
}
