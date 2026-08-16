// The inbox rail's entries: the feed, the paint, the four things a row can do —
// open, Done, mute, and say why it failed — and the one disclosure at the end
// of the list, Recent.
//
// WHICH rows appear and what they say is core/inbox.js; this module is the
// wiring. Read state is the bridge's now (`entity.seen`), so opening an entry
// tells the daemon, and nothing about what has been read is kept on the device.

import { $ } from "../dom.js";
import { App } from "../app.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { confirmAction } from "./confirm.js";
import {
  activeEntryKey,
  branchDoneConfirm,
  dismissParamsOf,
  entryKeyOf,
  inboxEntries,
  inboxListHtml,
  issueDoneConfirm,
} from "./inbox.js";
import { goFromInbox } from "./inboxShell.js";
import { branchOptions, mergeCaptureRows } from "./compose.js";
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
// Row keys a verb from this client just took off the list — Done, or a clear
// the daemon has not confirmed yet. The feed's own truth takes over as soon as
// it arrives. Keys, not entity ids: every row has a key, and the rows no
// entity stands behind (the primary checkout) can be cleared like any other.
const locallyHidden = new Set();
const busy = new Set(); // row keys with a mutation in flight
const errors = new Map(); // row key → the message its row is showing
const captureErrors = new Map(); // capture id → the message its row is showing

const messageOf = (error) => (error instanceof Error ? error.message : String(error));

/**
 * Tell the bridge this entry has been read. No agent id means the whole entry —
 * which is what opening it means; a bubble passes its own agent.
 *
 * This is also the hook for a self-initiated ending: merge and abandon are
 * attention-class events, so a merge the user triggered from this client would
 * otherwise badge its own entry. Whoever runs that verb calls this after it.
 */
export async function markSeen(entityId, agentId) {
  if (!entityId || !App.call) return;
  try {
    await App.call("entity.seen", agentId ? { entity_id: entityId, agent_id: agentId } : { entity_id: entityId });
  } catch {
    /* the cursor is the daemon's; a failed clear is re-tried by the next open */
  }
}

/** The entries a mutation from this client just ended, cleared in one call. */
export function noteSelfAction(...entityIds) {
  return Promise.all([...new Set(entityIds.filter(Boolean))].map((id) => markSeen(id)));
}

/** A box in the list has the caret. The rows are rewritten whole, so a repaint
 *  now would replace the box being typed into — taking the words, the caret and,
 *  on a phone, the keyboard with it. A tick nobody asked for stands down until
 *  the caret leaves; the user's own actions still repaint. */
function typingInList() {
  const active = document.activeElement;
  return Boolean(active && active.tagName === "INPUT" && active.closest("#inbox-list"));
}

/** The repaint the feed asks for, which is the one that must wait. */
function drawFromFeed() {
  if (typingInList()) return;
  draw();
}

function draw() {
  const list = $("#inbox-list");
  if (!list) return;
  // The captures this client is holding or watching stand beside the daemon's
  // own rows; the daemon's copy wins wherever both name the same capture.
  const partition = inboxEntries({
    items: mergeCaptureRows(items, pendingCaptureRows()),
    nowMs: Date.now(),
    hiddenKeys: locallyHidden,
  });
  // Every row on screen, Recent included: what the route stands on and what a
  // click resolves to do not care which section a row sits in.
  entries = [...partition.entries, ...partition.recent];
  const scroll = list.scrollTop;
  list.innerHTML = inboxListHtml(partition, {
    activeKey: activeEntryKey(App.route, entries),
    openMenuKey,
    recentOpen,
    rerouteKey,
    projects,
    rerouteBranchProject,
    // The branches that project already has, off the same feed rows the
    // compose panel offers: one source for "which branches are there".
    rerouteBranches: branchOptions(items, rerouteBranchProject),
  });
  list.scrollTop = scroll;
  wire(list);
  wireRecent(list);
  wireCaptures(list);
  paintErrors(list);
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

function wire(list) {
  list.querySelectorAll(".inbox-entry").forEach((row) => {
    row.onclick = (event) => {
      // The row's own controls answer for themselves.
      if (event.target.closest("[data-done], [data-menu], [data-mute], [data-dismiss], .inbox-actions")) return;
      openEntry(entryOf(row.dataset.key));
    };
  });
  list.querySelectorAll("[data-done]").forEach((control) => {
    control.onclick = (event) => {
      event.stopPropagation();
      closeMenu();
      finishEntry(entryOf(control.dataset.done));
    };
  });
  list.querySelectorAll("[data-mute]").forEach((control) => {
    control.onclick = (event) => {
      event.stopPropagation();
      toggleMute(entryOf(control.dataset.mute));
    };
  });
  list.querySelectorAll("[data-dismiss]").forEach((control) => {
    control.onclick = (event) => {
      event.stopPropagation();
      dismissEntry(entryOf(control.dataset.dismiss));
    };
  });
  list.querySelectorAll("[data-menu]").forEach((control) => {
    control.onclick = (event) => {
      event.stopPropagation();
      openMenuKey = openMenuKey === control.dataset.menu ? null : control.dataset.menu;
      draw();
      if (openMenuKey !== null) {
        const close = (outside) => {
          if (!outside.target.closest(".inbox-actions")) {
            document.removeEventListener("pointerdown", close);
            closeMenu();
          }
        };
        setTimeout(() => document.addEventListener("pointerdown", close), 0);
      }
    };
  });
}

/** Recent is one disclosure, and pressing it is the user saying so — from then
 *  on the section stays as they left it, whatever the list above it does. */
function wireRecent(list) {
  const toggle = list.querySelector("[data-recent-toggle]");
  if (!toggle) return;
  toggle.onclick = () => {
    recentOpen = toggle.getAttribute("aria-expanded") !== "true";
    draw();
  };
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

function wireCaptures(list) {
  list.querySelectorAll("[data-capture-retry]").forEach((control) => {
    control.onclick = (event) => {
      event.stopPropagation();
      rerouteCapture(control.dataset.captureRetry, null);
    };
  });
  list.querySelectorAll("[data-capture-reroute]").forEach((control) => {
    control.onclick = (event) => {
      event.stopPropagation();
      const key = `capture:${control.dataset.captureReroute}`;
      rerouteKey = rerouteKey === key ? null : key;
      rerouteBranchProject = null;
      draw();
    };
  });
  // Branch is the one destination with something left to say, so it discloses
  // the field that says it instead of dispatching on the spot.
  list.querySelectorAll("[data-reroute-branch-open]").forEach((control) => {
    control.onclick = (event) => {
      event.stopPropagation();
      const projectId = control.dataset.rerouteBranchOpen;
      rerouteBranchProject = rerouteBranchProject === projectId ? null : projectId;
      draw();
      // The field is found through the row that was just painted, never through
      // a selector built out of an id the daemon minted.
      if (rerouteBranchProject) $("#inbox-list")?.querySelector("[data-reroute-branch]")?.focus();
    };
  });
  list.querySelectorAll("[data-reroute-project]").forEach((control) => {
    control.onclick = (event) => {
      event.stopPropagation();
      const row = control.closest(".capture-entry");
      const named = control.dataset.rerouteKind === "branch" ? branchFieldValue(control) : "";
      rerouteKey = null;
      rerouteBranchProject = null;
      rerouteCapture(row.dataset.capture, {
        projectId: control.dataset.rerouteProject,
        kind: control.dataset.rerouteKind,
        branch: named,
      });
    };
  });
  list.querySelectorAll("[data-reroute-branch]").forEach((field) => {
    field.onkeydown = (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      field.closest(".reroute-branch").querySelector("[data-reroute-kind='branch']").click();
    };
  });
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
  if (busy.has(captureId)) return;
  busy.add(captureId);
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
    busy.delete(captureId);
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
  if (!entry || busy.has(entry.key)) return;
  busy.add(entry.key);
  openMenuKey = null;
  errors.delete(entry.key);
  try {
    await App.call("entity.mute", { entity_id: entry.entityId, muted: !entry.muted });
    await refreshFeed();
  } catch (error) {
    errors.set(entry.key, messageOf(error));
  } finally {
    busy.delete(entry.key);
    draw();
  }
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
  if (!params || busy.has(entry.key)) return;
  busy.add(entry.key);
  openMenuKey = null;
  locallyHidden.add(entry.key);
  errors.delete(entry.key);
  draw();
  try {
    // Clearing an unread row IS reading it: the daemon's rule keeps an unread
    // row visible (unread beats dismissed), so the tap reads it through first
    // — otherwise the row would bounce back on the next poll and Clear would
    // look broken on exactly the rows people most want to clear.
    if (entry.state === "unread" && entry.entityId) {
      await App.call("entity.seen", { entity_id: entry.entityId });
    }
    await App.call("entity.dismiss", params);
    await refreshFeed();
  } catch (error) {
    locallyHidden.delete(entry.key);
    errors.set(entry.key, messageOf(error));
  } finally {
    busy.delete(entry.key);
    draw();
  }
}

/** The RPC behind Done. On a branch it DELETES: the branch, its checkout and
 *  its records go, which is what Done on a branch means. On an issue it
 *  archives. Neither is refused for the state of the work — what the
 *  destruction costs came down with the row and was confirmed through. */
function finishCall(entry) {
  if (entry.kind === "issue") return App.call("plan.archive", { plan_id: entry.issueId });
  return App.call("branch.finish", { project_id: entry.projectId, branch: entry.branch, action: "delete" });
}

async function finishEntry(entry) {
  if (!entry || busy.has(entry.key)) return;
  const confirmation = entry.kind === "issue" ? issueDoneConfirm(entry) : branchDoneConfirm(entry);
  if (!(await confirmAction(confirmation))) return;
  busy.add(entry.key);
  // Confirmation is the decisive moment: the row goes now, and the git work
  // (and the feed catching up) carries on behind it.
  locallyHidden.add(entry.key);
  errors.delete(entry.key);
  draw();
  try {
    await finish(entry);
  } catch (error) {
    restore(entry, messageOf(error));
  } finally {
    busy.delete(entry.key);
  }
}

async function finish(entry) {
  await finishCall(entry);
  // Done ends the work, and an ending is an attention event. The user did this
  // here, so this entry is already read. The issue an unmerged branch leaves
  // behind is NOT: it comes back to the inbox asking for somebody, and the
  // event naming the branch it lost is the whole point of it coming back.
  await noteSelfAction(entry.entityId);
  await refreshFeed();
}

function restore(entry, message) {
  locallyHidden.delete(entry.key);
  errors.set(entry.key, message);
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
  subscribeFeed((feed) => {
    items = feed.items || [];
    projects = feed.projects || [];
    // A stale poll while git cleanup runs keeps a row this client removed
    // hidden. The daemon has caught up once the row is gone from the feed (Done)
    // or the feed itself calls it cleared (dismiss) — and from then on the
    // feed's `dismissed` alone decides, so a new event can revive the row.
    const live = new Map();
    for (const row of items) live.set(entryKeyOf(row), row);
    for (const key of locallyHidden) {
      const row = live.get(key);
      if (!row || row.dismissed) locallyHidden.delete(key);
    }
    for (const key of errors.keys()) if (!live.has(key)) errors.delete(key);
    drawFromFeed();
  });
}

/** Repaint so the entry the route stands on is the marked one. */
export function inboxListRouteChanged() {
  if (mounted) draw();
}
