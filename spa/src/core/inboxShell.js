// The inbox rail's chrome: whether it is docked or away, and the account entry
// at its foot. The entries themselves are the inbox view's business; this
// module only owns the rail as a piece of the shell.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
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
  document.body.classList.toggle("inbox-collapsed", on);
  localStorage.setItem(COLLAPSED_KEY, on ? "1" : "");
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
  $("#inbox-collapse").onclick = () => setInboxCollapsed(true);
  $("#inbox-open").onclick = () => setInboxCollapsed(false);
  $("#inbox-scrim").onclick = () => setInboxCollapsed(true);
  inboxRouteChanged();
}

/** Keep the rail tracking the route (the account entry today; the entry list
 *  once the inbox view owns it). */
export function inboxRouteChanged() {
  const account = $("#nav-account");
  if (account) account.classList.toggle("active", App.route.name === "account");
}
