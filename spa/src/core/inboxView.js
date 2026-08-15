// The inbox rail's entries: the feed, the paint, and the four things a row can
// do — open, Done, mute, and say why it failed.
//
// WHICH rows appear and what they say is core/inbox.js; this module is the
// wiring. Read state is the bridge's now (`entity.seen`), so opening an entry
// tells the daemon, and nothing about what has been read is kept on the device.

import { $ } from "../dom.js";
import { App } from "../app.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { confirmAction } from "./confirm.js";
import { entityIdOf } from "./entityId.js";
import {
  activeEntryKey,
  branchDoneConfirm,
  inboxEntries,
  inboxListHtml,
  issueDoneConfirm,
  unlinkDisclosure,
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
const dismissed = new Set(); // entity ids the user just said Done to
const busy = new Set(); // entity ids with a mutation in flight
const errors = new Map(); // entity id → the message its row is showing
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
  entries = inboxEntries({ items: mergeCaptureRows(items, pendingCaptureRows()), nowMs: Date.now(), dismissed });
  const scroll = list.scrollTop;
  list.innerHTML = inboxListHtml(entries, {
    activeKey: activeEntryKey(App.route, entries),
    openMenuKey,
    rerouteKey,
    projects,
    rerouteBranchProject,
    // The branches that project already has, off the same feed rows the
    // compose panel offers: one source for "which branches are there".
    rerouteBranches: branchOptions(items, rerouteBranchProject),
  });
  list.scrollTop = scroll;
  wire(list);
  wireCaptures(list);
  paintErrors(list);
}

/** An entity id is whatever the daemon minted (a worktree's is derived from a
 *  path), so rows are matched by reading their id back, never by building a
 *  selector out of it. */
function paintErrors(list) {
  list.querySelectorAll(".inbox-entry").forEach((row) => {
    const message = errors.get(row.dataset.entity) || captureErrors.get(row.dataset.capture);
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
      if (event.target.closest("[data-done], [data-menu], [data-mute], .inbox-actions")) return;
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
  if (!entry || busy.has(entry.entityId)) return;
  busy.add(entry.entityId);
  openMenuKey = null;
  errors.delete(entry.entityId);
  try {
    await App.call("entity.mute", { entity_id: entry.entityId, muted: !entry.muted });
    await refreshFeed();
  } catch (error) {
    errors.set(entry.entityId, messageOf(error));
  } finally {
    busy.delete(entry.entityId);
    draw();
  }
}

/** The RPC behind Done. A branch is finishable only once it is committed and
 *  pushed, so the checkout it leaves behind is always clean — `cleanup` is the
 *  only action that fits, and the archive is the branch itself. */
function finishCall(entry, unlink) {
  if (entry.kind === "issue") return App.call("plan.archive", { plan_id: entry.issueId });
  const params = { project_id: entry.projectId, branch: entry.branch, action: "cleanup" };
  return App.call("branch.finish", unlink ? { ...params, unlink: true } : params);
}

/** The refusal that IS the disclosure: the bridge will not archive an issue its
 *  branch has not implemented, and its error names the override. */
const isUnlinkRefusal = (message) => message.includes("unlink");

async function finishEntry(entry) {
  if (!entry || busy.has(entry.entityId)) return;
  const confirmation = entry.kind === "issue" ? issueDoneConfirm(entry) : branchDoneConfirm(entry);
  if (!(await confirmAction(confirmation))) return;
  busy.add(entry.entityId);
  // Confirmation is the decisive moment: the row goes now, and the git work
  // (and the feed catching up) carries on behind it.
  dismissed.add(entry.entityId);
  errors.delete(entry.entityId);
  draw();
  try {
    await finish(entry, false);
  } catch (error) {
    const message = messageOf(error);
    if (entry.kind === "branch" && isUnlinkRefusal(message) && (await confirmAction(unlinkDisclosure(entry, message)))) {
      try {
        await finish(entry, true);
      } catch (retry) {
        restore(entry, messageOf(retry));
      }
    } else {
      restore(entry, message);
    }
  } finally {
    busy.delete(entry.entityId);
  }
}

async function finish(entry, unlink) {
  await finishCall(entry, unlink);
  // Done ends the work, and an ending is an attention event. The user did this
  // here, so the entry (and the issue it archives with it) is already read.
  await noteSelfAction(entry.entityId, unlink ? null : entry.issueId);
  await refreshFeed();
}

function restore(entry, message) {
  dismissed.delete(entry.entityId);
  errors.set(entry.entityId, message);
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
    // A stale poll while git cleanup runs keeps the dismissed row hidden. Once
    // a feed no longer carries it, the daemon has caught up and the suppression
    // (and any error it left) can be forgotten.
    const live = new Set(items.map(entityIdOf).filter(Boolean));
    for (const entityId of dismissed) if (!live.has(entityId)) dismissed.delete(entityId);
    for (const entityId of errors.keys()) if (!live.has(entityId)) errors.delete(entityId);
    drawFromFeed();
  });
}

/** Repaint so the entry the route stands on is the marked one. */
export function inboxListRouteChanged() {
  if (mounted) draw();
}
