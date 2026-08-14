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
import "../styles/shell.css";

let items = [];
let entries = [];
let openMenuKey = null;
const dismissed = new Set(); // entity ids the user just said Done to
const busy = new Set(); // entity ids with a mutation in flight
const errors = new Map(); // entity id → the message its row is showing

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

function draw() {
  const list = $("#inbox-list");
  if (!list) return;
  entries = inboxEntries({ items, nowMs: Date.now(), dismissed });
  const scroll = list.scrollTop;
  list.innerHTML = inboxListHtml(entries, {
    activeKey: activeEntryKey(App.route, entries),
    openMenuKey,
  });
  list.scrollTop = scroll;
  wire(list);
  paintErrors(list);
}

/** An entity id is whatever the daemon minted (a worktree's is derived from a
 *  path), so rows are matched by reading their id back, never by building a
 *  selector out of it. */
function paintErrors(list) {
  if (!errors.size) return;
  list.querySelectorAll(".inbox-entry").forEach((row) => {
    const message = errors.get(row.dataset.entity);
    const slot = message && row.querySelector("[data-done-error]");
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
      if (event.target.closest("[data-done], [data-menu], [data-mute]")) return;
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
  subscribeFeed((feed) => {
    items = feed.items || [];
    // A stale poll while git cleanup runs keeps the dismissed row hidden. Once
    // a feed no longer carries it, the daemon has caught up and the suppression
    // (and any error it left) can be forgotten.
    const live = new Set(items.map(entityIdOf).filter(Boolean));
    for (const entityId of dismissed) if (!live.has(entityId)) dismissed.delete(entityId);
    for (const entityId of errors.keys()) if (!live.has(entityId)) errors.delete(entityId);
    draw();
  });
}

/** Repaint so the entry the route stands on is the marked one. */
export function inboxListRouteChanged() {
  if (mounted) draw();
}
