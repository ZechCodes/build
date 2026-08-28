// The inbox rail's chrome: whether it is docked or away, and the account entry
// at its foot. The entries themselves are the inbox view's business; this
// module only owns the rail as a piece of the shell.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { inboxListRouteChanged, mountInboxList } from "./inboxView.js";
import "../styles/shell.css";

const COLLAPSED_KEY = "build.inbox.collapsed";

/** Docked or away on this device, given what the user last chose and how much
 *  room there is. A viewport narrow enough to overlay the rail starts with it
 *  away; a choice, once made, is what counts. */
export function railStartsCollapsed(stored, viewportWidth) {
  if (stored === "1") return true;
  if (stored === "") return false;
  return viewportWidth < 900;
}

export function setInboxCollapsed(on) {
  setInboxPeek(false);
  if (on) collapsedAt = Date.now();
  document.body.classList.toggle("inbox-collapsed", on);
  localStorage.setItem(COLLAPSED_KEY, on ? "1" : "");
}

/* The hover peek: the pointer resting on the reopen toggle lays the collapsed
   rail over the view; leaving both the toggle and the rail puts it away. The
   close waits a beat because the rail, appearing over the toggle, hands the
   pointer from one to the other as a leave-then-enter pair. */
const PEEK_CLOSE_DELAY_MS = 150;
let peekCloseTimer = null;

/* Collapsing puts the floating toggle where the head toggle was — under the
   pointer that just clicked. The browser re-hit-tests and fires mouseenter on
   it, which would peek the rail right back open; a hover that soon after a
   collapse is that artifact, not a request. */
const PEEK_AFTER_COLLAPSE_MS = 300;
let collapsedAt = 0;

function peekWanted() {
  return Date.now() - collapsedAt > PEEK_AFTER_COLLAPSE_MS;
}

function setInboxPeek(on) {
  clearTimeout(peekCloseTimer);
  peekCloseTimer = null;
  document.body.classList.toggle("inbox-peek", on);
}

function schedulePeekClose() {
  clearTimeout(peekCloseTimer);
  peekCloseTimer = setTimeout(() => setInboxPeek(false), PEEK_CLOSE_DELAY_MS);
}

/** Navigating from the rail on a narrow viewport puts the rail away, so the
 *  destination isn't hidden behind it. On a wide one the rail is docked and
 *  stays put. */
export function goFromInbox(route) {
  go(route);
  if (window.innerWidth < 900) setInboxCollapsed(true);
}

let mounted = false;

/** Mount once. Re-entrant: a reconnect calls this again and it just repaints. */
export function initInboxRail() {
  if (mounted) {
    inboxRouteChanged();
    return;
  }
  mounted = true;
  setInboxCollapsed(railStartsCollapsed(localStorage.getItem(COLLAPSED_KEY), window.innerWidth));
  // One toggle in two places: the head button docks or puts the rail away, and
  // the floating one — at the same spot, while the rail is away — docks it.
  $("#inbox-collapse").onclick = () =>
    setInboxCollapsed(!document.body.classList.contains("inbox-collapsed"));
  const open = $("#inbox-open");
  open.onclick = () => setInboxCollapsed(false);
  open.onmouseenter = () => {
    if (peekWanted()) setInboxPeek(true);
  };
  open.onmouseleave = schedulePeekClose;
  const rail = $("#inbox-rail");
  rail.onmouseenter = () => {
    if (document.body.classList.contains("inbox-peek")) setInboxPeek(true);
  };
  rail.onmouseleave = () => {
    if (document.body.classList.contains("inbox-peek")) schedulePeekClose();
  };
  $("#inbox-scrim").onclick = () => setInboxCollapsed(true);
  mountInboxList();
  inboxRouteChanged();
}

/** Keep the rail tracking the route: the account entry at its foot, and the
 *  entry the route is standing on in the list. */
export function inboxRouteChanged() {
  const account = $("#nav-account");
  if (account) account.classList.toggle("active", App.route.name === "account");
  inboxListRouteChanged();
}
