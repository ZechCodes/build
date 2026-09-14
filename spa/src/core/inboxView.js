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
  branchDoneConfirm,
  captureEntries,
  dismissParamsOf,
  entryKeyOf,
  inboxEmptyHtml,
  inboxRowHtml,
  issueDoneConfirm,
  mergePendingRows,
  recentIsOpen,
  recentToggleHtml,
  workspaceEntries,
  workspaceEntryKey,
} from "./inbox.js";
import { patchList } from "./patchList.js";
import { BRANCH_DONE_OPTION, branchFinishFailureSummary, branchFinishParams } from "./branchFinish.js";
import {
  isPending,
  patchRecord,
  projectOptimistic,
  reconcileOptimistic,
  removeRecord,
  runOptimistic,
  subscribeOptimistic,
} from "./optimistic.js";
import { patchElement } from "./domPatch.js";
import { goFromInbox } from "./inboxShell.js";
import { routeProjectKey, routeWorkspaceKey } from "./deviceKey.js";
import { indexRowsByEntity, markSeen, noteSelfAction } from "./inboxSeen.js";
import { deviceFeedView, onDeviceStateChanged } from "./deviceContexts.js";
import { filterByDevice, onlyDeviceRows } from "./deviceFilter.js";
import { creationTarget, paintDeviceState, verbCall } from "./inboxDevices.js";
import { CAPTURE_CONTROLS, captureError, initCaptureRows, onCaptureKeydown, reroutePicker } from "./inboxCaptures.js";
import { projectRoute } from "./projectModel.js";
import {
  blockIsFolded,
  projectBlockHtml,
  projectHeadHtml,
  rowDeviceNames,
  workspaceProjectBlocks,
} from "./inboxProjects.js";
import { loadProjectFolds, persistProjectFolds } from "./railMode.js";
import { openCreateWork } from "./createWork.js";
import { openNewRepo } from "../sheets/newRepo.js";
import { branchOptions, mergeCaptureRows } from "./compose.js";
import { pendingCaptureRows, subscribePendingCaptures } from "./composeView.js";
import "../styles/shell.css";
import { publishInboxAttentionCount } from "./inboxAttention.js";
import { messageOf } from "./text.js";

let items = [];
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
// Each block as last painted, by project key: where its head opens, what it is
// called — which is what the create it offers is titled with — and the bare
// project id every RPC still wants.
let blocksPainted = new Map();
const errors = new Map(); // row key → the message its row is showing
const workspacesBeingFinished = new Set();


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

function publishAttentionCount() {
  const unread = workspaceEntries(workspaces, projects, items).filter((entry) => entry.state === "unread");
  publishInboxAttentionCount(new Set(unread.map((entry) => entry.entityId || entry.key)).size);
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
  draw();
}

function draw() {
  publishAttentionCount();
  const list = $("#inbox-list");
  if (!list) return;
  // The captures first: they are the account's unfinished business and belong
  // to no project, so they stand above the workspace rows on the flat face and
  // above the blocks on the other.
  const rows = projectOptimistic(INBOX_SCOPE, mergedItems(), { keyOf: entryKeyOf });
  const shown = withDeviceNames([...captureEntries(rows), ...workspaceEntries(workspaces, projects, rows)]);
  list.onclick = onListClick;
  list.onkeydown = onCaptureKeydown;
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
  list.querySelector(":scope > .inbox-recent")?.remove();
  list.querySelector(":scope > .inbox-unsorted")?.remove();
  list.querySelector(":scope > .inbox-projects")?.remove();
  patchList(list, entries, { keyOf, render: (entry) => inboxRowHtml(entry, ui) });
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
    // The block holding the branch or issue the route stands on. A capture's
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

/** The projects face: any workspaces whose project nothing lists, then one
 *  block per project — every machine's, each head naming its machine where two
 *  machines use that project name. */
function drawProjects(list, shown) {
  const { unsorted, blocks } = workspaceProjectBlocks(shown, projects, routeWorkspaceKey(App.route), App.devices);
  entries = [...unsorted, ...blocks.flatMap((block) => [...block.entries, ...block.recent])];
  blocksPainted = new Map(blocks.map((block) => [block.projectKey, block]));
  const folded = new Set(blocks.filter((block) => blockIsFolded(block, folds)).map((block) => block.projectKey));
  const ui = { ...rowUi(false), folded };
  const frame = projectsFrame(list);
  patchList(frame.unsorted, unsorted, { keyOf, render: (entry) => inboxRowHtml(entry, ui) });
  paintBlocks(frame.blocks, blocks, ui);
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
  openMenuKey = null;
  draw();
}

/** One control per attribute a row paints, in the order a press is read in:
 *  the innermost control wins. Each is handed the element that was pressed. */
const ROW_CONTROLS = [
  ["data-workspace-done", (control) => finishWorkspace(entryOf(control.dataset.workspaceDone))],
  ["data-done", (control) => finishRow(control.dataset.done)],
  ["data-mute", (control) => toggleMute(entryOf(control.dataset.mute))],
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
  draw();
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
  ["data-project-create", (control) => createInBlock(control.dataset.projectCreate)],
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
  // A control the row's device cannot answer for is shut, not hidden: the
  // reader can see the verb and reads why it is unavailable on it.
  if (target.closest('[aria-disabled="true"]')) return;
  if (pressed(LIST_CONTROLS, target)) return;
  // The row's own controls answer for themselves; everything else on it opens.
  const row = target.closest(".inbox-entry");
  if (row && !target.closest(".inbox-actions")) openEntry(entryOf(row.dataset.key));
}

/** The row's menu, one step behind the row: it opens, and the next press
 *  anywhere outside it shuts it again. */
function openMenu(key) {
  openMenuKey = openMenuKey === key ? null : key;
  draw();
  if (openMenuKey === null) return;
  const close = (outside) => {
    if (outside.target.closest(".inbox-actions")) return;
    document.removeEventListener("pointerdown", close);
    closeMenu();
  };
  setTimeout(() => document.addEventListener("pointerdown", close), 0);
}

// ---- project blocks -----------------------------------------------------------
//
// What a block's head can do: fold, open the project's checkout, and create —
// a branch or an issue, on the one create surface, scoped to the block's
// project. And the one control above every block: a new project. Which press
// is which is the table BLOCK_CONTROLS, up with the other control tables.

/** The block's name opens the project's workspace, when it has one. */
function openBlockHead(projectKey) {
  const block = blockOf(projectKey);
  if (block && block.route) goFromInbox(block.route);
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

/** The one control above every block, and the rail head's own: a project the
 *  account does not have yet. It is made where creation goes, over that
 *  machine's own connection; while no machine can answer there is nowhere to
 *  make it, and the rail says so. */
export function openNewProject() {
  const target = creationTarget("No device can take a new project");
  if (!target) return;
  openNewRepo((project) => {
    const route = projectRoute(project);
    if (route) goFromInbox(route);
    refreshFeed();
  }, target);
}

/** A fold is the user's, and it holds: across the feed, and across reloads. */
function toggleFold(projectKey) {
  const block = blockOf(projectKey);
  if (!block) return;
  folds.set(projectKey, !blockIsFolded(block, folds));
  persistProjectFolds(folds, localStorage);
  draw();
}

/** Creating a branch gives the new row somewhere visible to land. */
function expandFold(projectKey) {
  const block = blockOf(projectKey);
  if (!block || !blockIsFolded(block, folds)) return;
  folds.set(projectKey, false);
  persistProjectFolds(folds, localStorage);
  draw();
}

/** Opening an entry reads it — every agent on it — and goes where it lives. */
function openEntry(entry) {
  if (!entry) return;
  markSeen(entry.entityId).then(refreshFeed);
  if (entry.route) goFromInbox(entry.route);
}

async function toggleMute(entry) {
  if (!entry || isPending(INBOX_SCOPE, entry.key)) return;
  const muted = !entry.muted;
  openMenuKey = null;
  errors.delete(entry.key);
  await runOptimistic({
    scope: INBOX_SCOPE,
    records: [patchRecord(entry.key, { muted })],
    call: () => verbCall(entry)("entity.mute", { entity_id: entry.entityId, muted }),
    failureSummary: `Couldn't ${muted ? "mute" : "unmute"} ${entry.branch || "this item"}`,
    onRevert: (error) => showRowError(entry.key, error),
  });
  await refreshFeed();
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
  if (!params || isPending(INBOX_SCOPE, entry.key)) return;
  openMenuKey = null;
  errors.delete(entry.key);
  await runOptimistic({
    scope: INBOX_SCOPE,
    records: [patchRecord(entry.key, { dismissed: true })],
    call: async () => {
      // Clearing an unread row also acknowledges its unread notification. The
      // dismissal controls placement, but preserving read state avoids an
      // obsolete badge if the entry later returns.
      if (entry.state === "unread" && entry.entityId) {
        await verbCall(entry)("entity.seen", { entity_id: entry.entityId });
      }
      await verbCall(entry)("entity.dismiss", params);
    },
    failureSummary: `Couldn't clear ${entry.branch || "this item"}`,
    onRevert: (error) => showRowError(entry.key, error),
  });
  await refreshFeed();
}

/** The RPC behind Done. On a branch it DELETES: the branch, its checkout and
 *  its records go, which is what Done on a branch means. On an issue it
 *  archives. Neither is refused for the state of the work — what the
 *  destruction costs came down with the row and was confirmed through. */
export async function finishWorkItem(target, optionId = BRANCH_DONE_OPTION) {
  const call = verbCall(target);
  if (target.kind === "issue") await call("plan.archive", { plan_id: target.issueId });
  else await call("branch.finish", branchFinishParams(optionId, { projectId: target.projectId, branch: target.branch }));
  // Done ends the work, and an ending is an attention event. The user did this
  // here, so this entry is already read. The issue an unmerged branch leaves
  // behind is NOT: it comes back to the inbox asking for somebody, and the
  // event naming the branch it lost is the whole point of it coming back.
  await noteSelfAction(target.entityId, target.issueEnded ? target.issueId : null);
}

async function finishEntry(entry) {
  if (!entry || isPending(INBOX_SCOPE, entry.key)) return;
  const confirmation = entry.kind === "issue" ? issueDoneConfirm(entry) : branchDoneConfirm(entry);
  if (!(await confirmAction(confirmation))) return;
  errors.delete(entry.key);
  // Confirmation is the decisive moment: the row goes now, and the git work
  // (and the feed catching up) carries on behind it.
  await runOptimistic({
    scope: INBOX_SCOPE,
    records: [removeRecord(entry.key)],
    call: () => finishWorkItem(entry),
    failureSummary: branchFinishFailureSummary(entry.branch),
    onRevert: (error) => showRowError(entry.key, error),
  });
  await refreshFeed();
}

/** Archive a clean workspace in one tap. The bridge rechecks cleanliness at
 * execution time, stops every agent it owns, and preserves the checkout. */
async function finishWorkspace(entry) {
  if (!entry || entry.kind !== "workspace" || !entry.clean || workspacesBeingFinished.has(entry.key)) return;
  workspacesBeingFinished.add(entry.key);
  errors.delete(entry.key);
  draw();
  try {
    await verbCall(entry)("workspace.finish", { workspace_id: entry.workspaceId, require_clean: true });
    workspaces = workspaces.filter((workspace) => workspace.workspaceKey !== entry.workspaceKey);
  } catch (error) {
    errors.set(entry.key, messageOf(error));
  } finally {
    workspacesBeingFinished.delete(entry.key);
    draw();
  }
  await refreshFeed();
}

function showRowError(key, error) {
  errors.set(key, messageOf(error));
  draw();
}

let mounted = false;

/** Mount once. Re-entrant: a reconnect calls this again and it just repaints. */
export function mountInboxList() {
  if (mounted) {
    draw();
    return;
  }
  mounted = true;
  folds = loadProjectFolds(localStorage);
  initCaptureRows({ onChange: draw, entryOf });
  // A machine going or coming back changes no row, so the feed never says it:
  // the rail hears it from the registry and repaints, greying what the lost
  // device holds and shutting the verbs that would have asked it.
  onDeviceStateChanged(draw);
  subscribePendingCaptures(drawFromFeed);
  subscribeOptimistic(INBOX_SCOPE, draw);
  subscribeFeed((next) => {
    // Which machines the rail lists is the picker's, and it is answered once,
    // here: everything below paints whatever this snapshot holds.
    snapshot = filterByDevice(next, App.deviceFilter);
    items = snapshot.items || [];
    pendingLifecycle = snapshot.pending || [];
    projects = snapshot.projects || [];
    workspaces = snapshot.workspaces || [];
    const live = new Set([
      ...items.map(entryKeyOf),
      ...workspaces.map(workspaceEntryKey),
    ]);
    for (const key of errors.keys()) if (!live.has(key)) errors.delete(key);
    const merged = mergedItems();
    // Every verb that names only an entity — a read report, a self-action —
    // finds its row, and so its device, through this.
    indexRowsByEntity(merged);
    reconcileOptimistic(INBOX_SCOPE, merged, { keyOf: entryKeyOf });
    drawFromFeed();
  });
}

/** Repaint so the entry the route stands on is the marked one. */
export function inboxListRouteChanged() {
  if (mounted) draw();
}
