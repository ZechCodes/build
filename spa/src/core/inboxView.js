// The inbox rail's entries: the feed, the paint, the four things a row can do —
// open, Done, mute, and say why it failed — and the one disclosure at the end
// of the list, Recent.
//
// WHICH rows appear and what they say is core/inbox.js; this module is the
// wiring. Read state is the bridge's now (`entity.seen`), so opening an entry
// tells the daemon, and nothing about what has been read is kept on the device.

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
  recentIsOpen,
  recentToggleHtml,
} from "./inbox.js";
import { patchList } from "./patchList.js";
import { BRANCH_DONE_OPTION, branchFinishParams } from "./branchFinish.js";
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
import { branchOptions, mergeCaptureRows } from "./compose.js";
import { createSingleFlight } from "./splitButton.js";
import { adoptCaptureRecord, pendingCaptureRows, subscribePendingCaptures } from "./composeView.js";
import "../styles/shell.css";

let items = [];
let projects = [];
let entries = [];
let openMenuKey = null;
let rerouteKey = null; // the capture row whose destination picker is open
let rerouteBranchProject = null; // the project in that picker whose branch field is open
// Whether Recent is open, once the user has said. Null means nobody has, and
// the partition decides for itself (it opens when the list above it is thin).
let recentOpen = null;
const rerouteFlight = createSingleFlight();
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
 * This is also the hook for a self-initiated ending: merge and abandon are
 * attention-class events, so a merge the user triggered from this client would
 * otherwise badge its own entry. Whoever runs that verb calls this after it.
 */
export async function markSeen(entityId, agentId, readFromSequence = null) {
  if (!entityId || !App.call) return;
  try {
    await App.call("entity.seen", {
      entity_id: entityId,
      ...(agentId ? { agent_id: agentId } : {}),
      ...(typeof readFromSequence === "number" ? { read_from_sequence: readFromSequence } : {}),
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
  if (typingInList()) return;
  draw();
}

/** The one name a row has, which is what the reconciler matches rows by. */
const keyOf = (entry) => entry.key;

export const INBOX_SCOPE = "inbox";

// The captures this client is holding or watching stand beside the daemon's
// own rows; the daemon's copy wins wherever both name the same capture.
const mergedItems = () => mergeCaptureRows(items, pendingCaptureRows());

function draw() {
  const list = $("#inbox-list");
  if (!list) return;
  const partition = inboxEntries({
    items: projectOptimistic(INBOX_SCOPE, mergedItems(), { keyOf: entryKeyOf }),
    nowMs: Date.now(),
  });
  // Every row on screen, Recent included: what the route stands on and what a
  // click resolves to do not care which section a row sits in.
  entries = [...partition.entries, ...partition.recent];
  const ui = {
    activeKey: activeEntryKey(App.route, entries),
    openMenuKey,
    rerouteKey,
    projects,
    rerouteBranchProject,
    // The branches that project already has, off the same feed rows the
    // compose panel offers: one source for "which branches are there".
    rerouteBranches: branchOptions(items, rerouteBranchProject),
  };
  list.onclick = onListClick;
  list.onkeydown = onListKeydown;
  const scroll = list.scrollTop;
  paintEmpty(list, entries.length === 0);
  patchList(list, partition.entries, { keyOf, render: (entry) => inboxRowHtml(entry, ui) });
  paintRecent(list, partition, ui);
  list.scrollTop = scroll;
  paintErrors(list);
}

/** The line the rail shows in place of rows when it is holding nothing. It is
 *  chrome, not a row, and it goes the moment there is a row — a row is placed
 *  after the last row, so nothing else may be sitting down there. */
function paintEmpty(list, empty) {
  const standing = list.querySelector(".inbox-clear");
  if (empty === Boolean(standing)) return;
  if (standing) standing.remove();
  else list.appendChild(el(inboxEmptyHtml()));
}

/** Recent: the disclosure at the end of the list, and behind it the rows that
 *  have gone quiet. It is its own container, so the quiet rows are its keyed
 *  children and each list reconciles only its own. */
function paintRecent(list, partition, ui) {
  if (!partition.recent.length) {
    list.querySelector(".inbox-recent")?.remove();
    return;
  }
  let section = list.querySelector(".inbox-recent");
  if (!section) {
    section = document.createElement("div");
    section.className = "inbox-recent";
    section.append(el(recentToggleHtml(partition.recent, false)));
  }
  // Recent follows the list proper. A row that arrives while nothing was keyed
  // above it lands after the section, so the section is put back at the end
  // whenever a paint has left something below it.
  if (list.lastElementChild !== section) list.appendChild(section);
  const open = recentIsOpen(partition, recentOpen);
  patchElement(section.querySelector("[data-recent-toggle]"), el(recentToggleHtml(partition.recent, open)));
  patchList(section, open ? partition.recent : [], { keyOf, render: (entry) => inboxRowHtml(entry, ui) });
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
    recentOpen = recentToggle.getAttribute("aria-expanded") !== "true";
    draw();
    return;
  }
  if (captureClicked(target)) return;
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
  if (!rerouteFlight.begin()) return;
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
    rerouteFlight.end();
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
    call: async () => {
      await App.call("entity.mute", { entity_id: entry.entityId, muted });
      await refreshFeed();
    },
    failureSummary: `Couldn't mute ${entry.branch || "this item"}`,
    onRevert: (error) => showRowError(entry.key, error),
  });
}

/** Clear the row off the inbox until something new needs the user. Nothing is
 *  destroyed, nothing is silenced: for a row with a conversation the daemon
 *  remembers how far it had got, and the next attention event past that brings
 *  the row back by itself; a row with none (a bare checkout, the primary) is
 *  cleared at the commit it sits on, and a new commit brings it back — so
 *  there is no un-clear verb to offer. The dismiss names the row the way
 *  dismissParamsOf says: an entity by its id, an entity-less row by what it is.
 *
 *  The row leaves on the tap and comes back if the daemon refuses. */
async function dismissEntry(entry) {
  const params = entry && dismissParamsOf(entry);
  if (!params || isPending(INBOX_SCOPE, entry.key)) return;
  openMenuKey = null;
  errors.delete(entry.key);
  await runOptimistic({
    scope: INBOX_SCOPE,
    records: [patchRecord(entry.key, { dismissed: true })],
    call: async () => {
      // Clearing an unread row IS reading it: the daemon's rule keeps an unread
      // row visible (unread beats dismissed), so the tap reads it through first
      // — otherwise the row would bounce back on the next poll and Clear would
      // look broken on exactly the rows people most want to clear.
      if (entry.state === "unread" && entry.entityId) {
        await App.call("entity.seen", { entity_id: entry.entityId });
      }
      await App.call("entity.dismiss", params);
      await refreshFeed();
    },
    failureSummary: `Couldn't clear ${entry.branch || "this item"}`,
    onRevert: (error) => showRowError(entry.key, error),
  });
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
  await refreshFeed();
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
    failureSummary: `Couldn't finish ${entry.branch || "this item"}`,
    onRevert: (error) => showRowError(entry.key, error),
  });
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
  subscribePendingCaptures(drawFromFeed);
  subscribeOptimistic(INBOX_SCOPE, draw);
  subscribeFeed((feed) => {
    items = feed.items || [];
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
