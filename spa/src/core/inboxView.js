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
  dismissParamsOf,
  entryKeyOf,
  inboxEmptyHtml,
  inboxEntries,
  inboxRowHtml,
  issueDoneConfirm,
  mergePendingRows,
  recentIsOpen,
  recentToggleHtml,
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
import { homeProjectKey } from "./deviceContexts.js";
import { projectRoute } from "./projectModel.js";
import { blockIsFolded, newProjectButtonHtml, projectBlockHtml, projectBlocks, projectHeadHtml } from "./inboxProjects.js";
import { loadProjectFolds, persistProjectFolds } from "./railMode.js";
import { openCreateWork } from "./createWork.js";
import { openNewRepo } from "../sheets/newRepo.js";
import { branchOptions, mergeCaptureRows } from "./compose.js";
import { adoptCaptureRecord, pendingCaptureRows, subscribePendingCaptures } from "./composeView.js";
import "../styles/shell.css";
import { publishInboxAttentionCount } from "./inboxAttention.js";

let items = [];
// The board's rows for lifecycle verbs in flight (board.list's `pending`): a
// checkout being cut is on the list while its git runs.
let pendingLifecycle = [];
let projects = [];
let entries = [];
let view = "inbox"; // which face the rail is showing: "inbox" or "projects"
let openMenuKey = null;
let rerouteKey = null; // the capture row whose destination picker is open
let rerouteBranchProject = null; // the project in that picker whose branch field is open
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
const capturesBeingRerouted = new Set();
const errors = new Map(); // row key → the message its row is showing
const captureErrors = new Map(); // capture id → the message its row is showing

const messageOf = (error) => (error instanceof Error ? error.message : String(error));

/**
 * Tell the bridge this entry has been read. No agent id means the whole entry —
 * which is what opening it means; a bubble passes its own agent.
 *
 * A reader who holds a WINDOW on a long conversation rather than the whole of
 * it passes the sequence that window starts at, so the daemon moves the read
 * cursor only as far as the reader was actually sent. No floor says what it
 * always said: the conversation arrived whole.
 *
 * `readThroughSequence` is the newest message the reader's viewport actually
 * reached. Reading is per message — a panel showing half of what arrived clears
 * half of it — and no sequence says the reader read to the end of what they
 * hold, which is what opening a whole entry means.
 *
 * This is also the hook for a self-initiated ending: merge and abandon are
 * attention-class events, so a merge the user triggered from this client would
 * otherwise badge its own entry. Whoever runs that verb calls this after it.
 */
export async function markSeen(entityId, agentId, readFromSequence = null, readThroughSequence = null) {
  if (!entityId || !App.call) return;
  try {
    await App.call("entity.seen", {
      entity_id: entityId,
      ...(agentId ? { agent_id: agentId } : {}),
      ...(typeof readFromSequence === "number" ? { read_from_sequence: readFromSequence } : {}),
      ...(typeof readThroughSequence === "number" ? { read_through_sequence: readThroughSequence } : {}),
    });
  } catch {
    /* the cursor is the daemon's; a failed clear is re-tried by the next open */
  }
}

/** The entries a mutation from this client just ended, cleared in one call. */
export function noteSelfAction(...entityIds) {
  return Promise.all([...new Set(entityIds.filter(Boolean))].map((id) => markSeen(id)));
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

function publishAttentionCount() {
  const shown = projectOptimistic(INBOX_SCOPE, mergedItems(), { keyOf: entryKeyOf });
  const partition = inboxEntries({ items: shown, nowMs: Date.now() });
  publishInboxAttentionCount(
    [...partition.entries, ...partition.recent]
      .filter((entry) => !entry.muted && !entry.dismissed)
      .reduce((total, entry) => total + entry.unreadCount, 0),
  );
}

/** The one name a row has, which is what the reconciler matches rows by. */
const keyOf = (entry) => entry.key;

export const INBOX_SCOPE = "inbox";

// The captures this client is holding or watching stand beside the daemon's
// own rows; the daemon's copy wins wherever both name the same capture.
const mergedItems = () => mergeCaptureRows(mergePendingRows(items, pendingLifecycle), pendingCaptureRows());

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
  const shown = projectOptimistic(INBOX_SCOPE, mergedItems(), { keyOf: entryKeyOf });
  const nowMs = Date.now();
  list.onclick = onListClick;
  list.onkeydown = onListKeydown;
  // A different face is a different list: the one is emptied for the other,
  // and every paint after that reconciles in place.
  if (list.dataset.view !== view) {
    list.dataset.view = view;
    list.replaceChildren();
  }
  const scroll = list.scrollTop;
  if (view === "projects") drawProjects(list, shown, nowMs);
  else drawInbox(list, shown, nowMs);
  list.scrollTop = scroll;
  paintErrors(list);
}

/** What every row is painted with. `showProject` is whether a row names its
 *  own project — under a project block it has already been told. */
function rowUi(showProject) {
  return {
    activeKey: activeEntryKey(App.route, entries),
    openMenuKey,
    rerouteKey,
    projects,
    rerouteBranchProject,
    // The branches that project already has, off the same feed rows the
    // compose panel offers: one source for "which branches are there".
    rerouteBranches: branchOptions(items, rerouteBranchProject),
    showProject,
    folded: new Set(),
    // The block holding the branch or issue the route stands on. A capture's
    // route names no project; the row it stands on does.
    activeProjectId: activeProjectKey(),
  };
}

/** The block the route stands in, named the way every block is named. The row
 *  the route opens says which device it is on; a route that matches no row is
 *  the home device's project. */
function activeProjectKey() {
  const standing = entries.find((entry) => entry.key === activeEntryKey(App.route, entries));
  if (standing) return standing.projectKey || null;
  return homeProjectKey(App.route.projectId);
}

/** The inbox face: one list, Recent at its end. */
function drawInbox(list, shown, nowMs) {
  const partition = inboxEntries({ items: shown, nowMs });
  // Every row on screen, Recent included: what the route stands on and what a
  // click resolves to do not care which section a row sits in.
  entries = [...partition.entries, ...partition.recent];
  const ui = rowUi(true);
  paintEmpty(list, entries.length === 0, inboxEmptyHtml, ".inbox-clear");
  patchList(list, partition.entries, { keyOf, render: (entry) => inboxRowHtml(entry, ui) });
  paintRecent(list, partition, ui, "inbox");
}

/** The projects face: the new-project control, the unrouted captures on their
 *  own, then a block per project with its rows and its own Recent. */
function drawProjects(list, shown, nowMs) {
  const face = projectBlocks({ items: shown, projects, devices: App.devices, nowMs });
  entries = [...face.unsorted, ...face.blocks.flatMap((block) => [...block.entries, ...block.recent])];
  blocksPainted = new Map(face.blocks.map((block) => [block.projectKey, block]));
  const folded = new Set(face.blocks.filter((block) => blockIsFolded(block, folds)).map((block) => block.projectKey));
  const ui = { ...rowUi(false), folded };
  const frame = projectsFrame(list);
  patchList(frame.unsorted, face.unsorted, { keyOf, render: (entry) => inboxRowHtml(entry, ui) });
  paintBlocks(frame.blocks, face.blocks, ui);
}

/** The projects face's frame, built once: the new-project control, the loose
 *  rows' container, and the blocks'. */
function projectsFrame(list) {
  let unsorted = list.querySelector(":scope > .inbox-unsorted");
  if (!unsorted) {
    unsorted = el('<div class="inbox-unsorted"></div>');
    list.append(el(newProjectButtonHtml()), unsorted, el('<div class="inbox-projects"></div>'));
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
      paintRecent(element, block, ui, block.projectKey);
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
function paintRecent(host, partition, ui, scope) {
  if (!partition.recent.length) {
    host.querySelector(":scope > .inbox-recent")?.remove();
    return;
  }
  let section = host.querySelector(":scope > .inbox-recent");
  if (!section) {
    section = document.createElement("div");
    section.className = "inbox-recent";
    section.append(el(recentToggleHtml(partition.recent, false, scope)));
  }
  // Recent follows the list proper. A row that arrives while nothing was keyed
  // above it lands after the section, so the section is put back at the end
  // whenever a paint has left something below it.
  if (host.lastElementChild !== section) host.appendChild(section);
  const open = recentIsOpen(recentOpen.get(scope));
  patchElement(section.querySelector("[data-recent-toggle]"), el(recentToggleHtml(partition.recent, open, scope)));
  // Recent's rows are quiet rows: one line each, no state dot.
  patchList(section, open ? partition.recent : [], { keyOf, render: (entry) => inboxRowHtml(entry, { ...ui, quiet: true }) });
}

/** A row key is whatever the daemon minted (a worktree's is derived from a
 *  path), so rows are matched by reading their key back, never by building a
 *  selector out of it. */
function paintErrors(list) {
  list.querySelectorAll(".inbox-entry").forEach((row) => {
    const message = errors.get(row.dataset.key) || captureErrors.get(row.dataset.capture);
    const slot = message && row.querySelector("[data-done-error], [data-capture-error]");
    if (!slot) return;
    slot.textContent = message;
    slot.hidden = false;
  });
}

const entryOf = (key) => entries.find((entry) => entry.key === key) || null;

function closeMenu() {
  if (openMenuKey === null) return;
  openMenuKey = null;
  draw();
}

/// Every control in the list, answered in one place.
///
/// A row's element survives the paints, but a control inside it does not have
/// to — Done appears the moment the work can be finished — so nothing is wired
/// to a row or to a button. The list itself listens, and reads off the DOM which
/// row was spoken for.
function onListClick(event) {
  const { target } = event;
  const done = target.closest("[data-done]");
  if (done) {
    closeMenu();
    finishEntry(entryOf(done.dataset.done));
    return;
  }
  const mute = target.closest("[data-mute]");
  if (mute) {
    toggleMute(entryOf(mute.dataset.mute));
    return;
  }
  const dismiss = target.closest("[data-dismiss]");
  if (dismiss) {
    dismissEntry(entryOf(dismiss.dataset.dismiss));
    return;
  }
  const menu = target.closest("[data-menu]");
  if (menu) {
    openMenu(menu.dataset.menu);
    return;
  }
  // Recent is one disclosure, and pressing it is the user saying so — from then
  // on the section stays as they left it, whatever the list above it does.
  const recentToggle = target.closest("[data-recent-toggle]");
  if (recentToggle) {
    recentOpen.set(recentToggle.dataset.recentToggle, recentToggle.getAttribute("aria-expanded") !== "true");
    draw();
    return;
  }
  if (captureClicked(target)) return;
  if (projectClicked(target)) return;
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
// project. And the one control above every block: a new project.

/** The block controls, answered off the same one listener. True when the press
 *  was one of them. */
function projectClicked(target) {
  const fold = target.closest("[data-project-fold]");
  if (fold) {
    toggleFold(fold.dataset.projectFold);
    return true;
  }
  const head = target.closest("[data-project-open]");
  if (head) {
    const block = blocksPainted.get(head.dataset.projectOpen);
    if (block && block.route) goFromInbox(block.route);
    return true;
  }
  const create = target.closest("[data-project-create]");
  if (create) {
    const block = blocksPainted.get(create.dataset.projectCreate);
    expandFold(create.dataset.projectCreate);
    closeMenu();
    // The create surface talks to one bridge, which knows its projects by the
    // bare id it minted.
    if (block) openCreateWork({ projectId: block.id, projectName: block.name, kind: "branch", navigate: goFromInbox });
    return true;
  }
  if (target.closest("[data-new-project]")) {
    openNewRepo((project) => {
      const route = projectRoute(project);
      if (route) goFromInbox(route);
      refreshFeed();
    });
    return true;
  }
  return false;
}

/** A fold is the user's, and it holds: across the feed, and across reloads. */
function toggleFold(projectKey) {
  const block = blocksPainted.get(projectKey);
  if (!block) return;
  folds.set(projectKey, !blockIsFolded(block, folds));
  persistProjectFolds(folds, localStorage);
  draw();
}

/** Creating a branch gives the new row somewhere visible to land. */
function expandFold(projectKey) {
  const block = blocksPainted.get(projectKey);
  if (!block || !blockIsFolded(block, folds)) return;
  folds.set(projectKey, false);
  persistProjectFolds(folds, localStorage);
  draw();
}

// ---- capture rows -------------------------------------------------------------
//
// The two things a row can do to a route: retry one that gave up, and send the
// capture somewhere else. Both go through the daemon's own capture verbs — a
// reroute by hand and a route by the router are the same kind of thing
// afterwards.
//
// Answering the router is not one of them. What to do with a capture is a
// decision with several shapes — the router's own choices, a destination named
// by hand, words, or abandoning it — and the row opens the page that holds all
// of them (views/captureDecision.js) rather than hosting the thinnest one.

/** The capture controls, answered off the same one listener. True when the press
 *  was one of them. */
function captureClicked(target) {
  const retry = target.closest("[data-capture-retry]");
  if (retry) {
    rerouteCapture(retry.dataset.captureRetry, null);
    return true;
  }
  const reroute = target.closest("[data-capture-reroute]");
  if (reroute) {
    const key = `capture:${reroute.dataset.captureReroute}`;
    rerouteKey = rerouteKey === key ? null : key;
    rerouteBranchProject = null;
    draw();
    return true;
  }
  // Branch is the one destination with something left to say, so it discloses
  // the field that says it instead of dispatching on the spot.
  const branchOpen = target.closest("[data-reroute-branch-open]");
  if (branchOpen) {
    openRerouteBranch(branchOpen.dataset.rerouteBranchOpen);
    return true;
  }
  const destination = target.closest("[data-reroute-project]");
  if (destination) {
    dispatchReroute(destination);
    return true;
  }
  return false;
}

function openRerouteBranch(projectId) {
  rerouteBranchProject = rerouteBranchProject === projectId ? null : projectId;
  draw();
  // The field is found through the list that was just painted, never through a
  // selector built out of an id the daemon minted.
  if (rerouteBranchProject) $("#inbox-list")?.querySelector("[data-reroute-branch]")?.focus();
}

function dispatchReroute(control) {
  const row = control.closest(".capture-entry");
  const named = control.dataset.rerouteKind === "branch" ? branchFieldValue(control) : "";
  rerouteKey = null;
  rerouteBranchProject = null;
  rerouteCapture(row.dataset.capture, {
    projectId: control.dataset.rerouteProject,
    kind: control.dataset.rerouteKind,
    branch: named,
  });
}

/** Enter in the branch field is the Dispatch beside it. */
function onListKeydown(event) {
  if (event.key !== "Enter") return;
  const field = event.target.closest("[data-reroute-branch]");
  if (!field) return;
  event.preventDefault();
  field.closest(".reroute-branch").querySelector("[data-reroute-kind='branch']").click();
}

/** The branch named beside a Dispatch button, "" when the field is empty or
 *  the destination was chosen without one. */
function branchFieldValue(control) {
  const field = control.closest(".reroute-branch")?.querySelector("[data-reroute-branch]");
  return field ? field.value.trim() : "";
}

/** With a destination this routes by hand; with none it re-fires the router,
 *  which is what the retry on a failed route is. */
async function rerouteCapture(captureId, destination) {
  if (capturesBeingRerouted.has(captureId)) return;
  capturesBeingRerouted.add(captureId);
  captureErrors.delete(captureId);
  try {
    const rerouted = await App.call("capture.reroute", rerouteParams(captureId, destination));
    // The answer carries the new routing, and for a capture that has already
    // settled it is the only thing that will: the feed stopped carrying it, so
    // nothing else would ever correct the row's "→ project as issue".
    adoptCaptureRecord(rerouted);
    await refreshFeed();
  } catch (error) {
    captureErrors.set(captureId, messageOf(error));
  } finally {
    capturesBeingRerouted.delete(captureId);
    draw();
  }
}

/** What a reroute asks for: a destination, or nothing at all — which is the
 *  retry, and means "decide again". A branch carries the name when one was
 *  given; with none the daemon names it after what was said. */
function rerouteParams(captureId, destination) {
  if (!destination) return { capture_id: captureId };
  const params = { capture_id: captureId, project_id: destination.projectId, kind: destination.kind };
  return destination.branch ? { ...params, branch: destination.branch } : params;
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
    call: () => App.call("entity.mute", { entity_id: entry.entityId, muted }),
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
        await App.call("entity.seen", { entity_id: entry.entityId });
      }
      await App.call("entity.dismiss", params);
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
  if (target.kind === "issue") await App.call("plan.archive", { plan_id: target.issueId });
  else await App.call("branch.finish", branchFinishParams(optionId, { projectId: target.projectId, branch: target.branch }));
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
  subscribePendingCaptures(drawFromFeed);
  subscribeOptimistic(INBOX_SCOPE, draw);
  subscribeFeed((feed) => {
    items = feed.items || [];
    pendingLifecycle = feed.pending || [];
    projects = feed.projects || [];
    const live = new Set(items.map(entryKeyOf));
    for (const key of errors.keys()) if (!live.has(key)) errors.delete(key);
    reconcileOptimistic(INBOX_SCOPE, mergedItems(), { keyOf: entryKeyOf });
    drawFromFeed();
  });
}

/** Repaint so the entry the route stands on is the marked one. */
export function inboxListRouteChanged() {
  if (mounted) draw();
}
