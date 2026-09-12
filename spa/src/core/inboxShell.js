// The inbox rail's chrome: whether it is docked or away, which of its two faces
// it is showing, and the account entry at its foot. The entries themselves are
// the inbox view's business; this module only owns the rail as a piece of the
// shell.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { inboxListRouteChanged, mountInboxList, openNewProject, setInboxView } from "./inboxView.js";
import { loadRailView, persistRailView, railViewSwitchHtml } from "./railMode.js";
import { subscribeInboxAttentionCount } from "./inboxAttention.js";
import { ICON_PIN, ICON_PLUS } from "./icons.js";
import "../styles/shell.css";

const COLLAPSED_KEY = "build.inbox.collapsed";

/** Docked or away on this device, given what the user last chose and how much
 *  room there is. A viewport narrow enough to overlay the rail starts with it
 *  away; a choice, once made, is what counts. */
export function railStartsCollapsed(stored, viewportWidth) {
  if (viewportWidth <= 900) return true;
  if (stored === "1") return true;
  if (stored === "") return false;
  return false;
}

export function setInboxCollapsed(on, { animate = true, persist = true, reveal = on } = {}) {
  const wasCollapsed = document.body.classList.contains("inbox-collapsed");
  setInboxPeek(false);
  if (on) collapsedAt = Date.now();
  const rail = $("#inbox-rail");
  const before = rail?.getBoundingClientRect();
  document.body.classList.toggle("inbox-collapsed", on);
  document.body.classList.toggle("inbox-popover-open", on && reveal);
  persistCollapsedChoice(on, persist);
  syncInboxControls(on);
  animateCollapsedChange(animate && wasCollapsed !== on, rail, before);
}

function persistCollapsedChoice(on, persist) {
  if (persist) localStorage.setItem(COLLAPSED_KEY, on ? "1" : "");
}

function animateCollapsedChange(changed, rail, before) {
  if (changed) animateInboxTransition(rail, before);
}

let transitionRun = 0;
let transitionCleanup = null;
const LAYOUT_TRANSITION_MS = 240;
const PANEL_TRANSITION_MS = 160;

function pinRailRect(rail, rect) {
  Object.assign(rail.style, {
    position: "fixed",
    left: `${rect.left}px`,
    top: `${rect.top}px`,
    width: `${rect.width}px`,
    height: `${rect.height}px`,
    maxHeight: "none",
    margin: "0",
    zIndex: "46",
  });
}

function releaseRailRect(rail) {
  for (const property of ["position", "left", "top", "width", "height", "max-height", "margin", "z-index"]) {
    rail.style.removeProperty(property);
  }
}

function animateInboxTransition(rail, before) {
  const run = ++transitionRun;
  transitionCleanup?.();
  transitionCleanup = null;
  if (!rail || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
  const after = rail.getBoundingClientRect();
  if (!before?.width || !after.width) return;
  document.body.classList.add("inbox-transitioning");
  pinRailRect(rail, before);
  let animation = null;
  let settleTimer = null;
  const finish = () => {
    if (run !== transitionRun) return;
    clearTimeout(settleTimer);
    releaseRailRect(rail);
    document.body.classList.remove("inbox-transitioning");
    transitionCleanup = null;
  };
  settleTimer = setTimeout(() => {
    if (run !== transitionRun) return;
    animation = rail.animate?.(
      [
        { left: `${before.left}px`, top: `${before.top}px`, width: `${before.width}px`, height: `${before.height}px` },
        { left: `${after.left}px`, top: `${after.top}px`, width: `${after.width}px`, height: `${after.height}px` },
      ],
      { duration: PANEL_TRANSITION_MS, easing: "cubic-bezier(.2,.8,.2,1)" },
    );
    if (!animation) return finish();
    pinRailRect(rail, after);
    animation.onfinish = finish;
    animation.oncancel = finish;
  }, LAYOUT_TRANSITION_MS);
  transitionCleanup = () => {
    clearTimeout(settleTimer);
    animation?.cancel();
    releaseRailRect(rail);
    document.body.classList.remove("inbox-transitioning");
  };
}

function syncInboxControls(collapsed = document.body.classList.contains("inbox-collapsed")) {
  const open = $("#inbox-open");
  const pin = $("#inbox-collapse");
  const visible = !collapsed || document.body.classList.contains("inbox-popover-open") || document.body.classList.contains("inbox-peek");
  if (open) syncOpenControl(open, visible, collapsed);
  if (pin) syncPinControl(pin, collapsed);
}

function syncOpenControl(open, visible, collapsed) {
  const count = open.dataset.attentionCount;
  const attention = count ? `, ${count} unread notification${count === "1" ? "" : "s"}` : "";
  const label = `${collapsed ? (visible ? "Close" : "Open") + " the inbox" : "Go to inbox"}${attention}`;
  open.setAttribute("aria-expanded", String(visible));
  open.setAttribute("aria-label", label);
  open.title = label;
}

function syncPinControl(pin, collapsed) {
  pin.setAttribute("aria-pressed", String(!collapsed));
  pin.setAttribute("aria-label", collapsed ? "Pin the inbox" : "Unpin the inbox");
  pin.title = collapsed ? "Pin the inbox" : "Unpin the inbox";
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
const HOVER_PEEK_QUERY = "(hover: hover) and (pointer: fine)";
let collapsedAt = 0;

function peekWanted() {
  return Date.now() - collapsedAt > PEEK_AFTER_COLLAPSE_MS;
}

function setInboxPeek(on) {
  clearTimeout(peekCloseTimer);
  peekCloseTimer = null;
  document.body.classList.toggle("inbox-peek", on);
  syncInboxControls();
}

function setInboxPopover(on, { restoreFocus = false } = {}) {
  if (!document.body.classList.contains("inbox-collapsed")) return;
  setInboxPeek(false);
  document.body.classList.toggle("inbox-popover-open", on);
  syncInboxControls();
  if (restoreFocus) $("#inbox-open")?.focus();
}

function dismissInbox({ restoreFocus = false } = {}) {
  const pinnedMobile = window.innerWidth <= 900 && !document.body.classList.contains("inbox-collapsed");
  if (pinnedMobile) setInboxCollapsed(true, { persist: false, reveal: false });
  else setInboxPopover(false, { restoreFocus });
  if (restoreFocus) $("#inbox-open")?.focus();
}

function schedulePeekClose() {
  clearTimeout(peekCloseTimer);
  peekCloseTimer = setTimeout(() => setInboxPeek(false), PEEK_CLOSE_DELAY_MS);
}

/** Hover peek belongs to a mouse or trackpad. Touch browsers synthesize mouse
 *  enter/leave events around a tap; letting those events move the rail over the
 *  touched control can remove its click target before activation arrives. */
function wireHoverPeek(open, rail) {
  const capability = window.matchMedia?.(HOVER_PEEK_QUERY);
  const hoverCapable = () => capability?.matches === true;
  const capabilityChanged = () => {
    if (!hoverCapable()) setInboxPeek(false);
  };

  open.onmouseenter = () => {
    if (hoverCapable() && peekWanted()) setInboxPeek(true);
  };
  open.onmouseleave = () => {
    if (hoverCapable()) schedulePeekClose();
  };
  rail.onmouseenter = () => {
    if (hoverCapable() && document.body.classList.contains("inbox-peek")) setInboxPeek(true);
  };
  rail.onmouseleave = () => {
    if (hoverCapable() && document.body.classList.contains("inbox-peek")) schedulePeekClose();
  };
  capability?.addEventListener?.("change", capabilityChanged);
}

/** Navigating from the rail on a narrow viewport puts the rail away, so the
 *  destination isn't hidden behind it. On a wide one the rail is docked and
 *  stays put. */
export function goFromInbox(route) {
  const navigation = go(route);
  const closeAfterNavigation = (accepted) => {
    if (accepted && window.innerWidth < 900) {
      setInboxCollapsed(true, { persist: false, reveal: false });
    }
    return accepted;
  };
  return navigation instanceof Promise ? navigation.then(closeAfterNavigation) : closeAfterNavigation(navigation);
}

/* The two faces — the one list, or the projects — behind the switch at the
   head's right edge. The choice is remembered on this device. */
function paintViewSwitch(view) {
  const host = $("#inbox-views");
  if (host) host.innerHTML = railViewSwitchHtml(view);
}

function chooseView(view) {
  persistRailView(view, localStorage);
  paintViewSwitch(view);
  setInboxView(view);
}

let mounted = false;

function paintAttentionCount(count) {
  const open = $("#inbox-open");
  if (!open) return;
  const hasAttention = count > 0;
  open.dataset.attentionCount = hasAttention ? String(count) : "";
  open.classList.toggle("has-attention", hasAttention);
  open.querySelector(".inbox-open-count").textContent = count > 99 ? "99+" : String(count || "");
  syncInboxControls();
}

/** Mount once. Re-entrant: a reconnect calls this again and it just repaints. */
export function initInboxRail() {
  if (mounted) {
    inboxRouteChanged();
    return;
  }
  mounted = true;
  const startsCollapsed = railStartsCollapsed(localStorage.getItem(COLLAPSED_KEY), window.innerWidth);
  setInboxCollapsed(startsCollapsed, { animate: false, persist: false, reveal: false });
  // One toggle in two places: the head button docks or puts the rail away, and
  // the floating one — at the same spot, while the rail is away — docks it.
  const pin = $("#inbox-collapse");
  pin.innerHTML = ICON_PIN;
  pin.onclick = () => {
    if (window.innerWidth <= 900) return;
    setInboxCollapsed(!document.body.classList.contains("inbox-collapsed"));
  };
  const newProject = $("#inbox-new-project");
  newProject.innerHTML = `${ICON_PLUS}<span>New project</span>`;
  newProject.onclick = () => {
    setInboxPeek(false);
    dismissInbox();
    openNewProject();
  };
  const open = $("#inbox-open");
  subscribeInboxAttentionCount(paintAttentionCount);
  open.onclick = () => {
    if (!document.body.classList.contains("inbox-collapsed")) return goFromInbox({ name: "inbox" });
    setInboxPopover(!document.body.classList.contains("inbox-popover-open"));
  };
  const rail = $("#inbox-rail");
  wireHoverPeek(open, rail);
  $("#inbox-scrim").onclick = () => dismissInbox();
  document.addEventListener("pointerdown", (event) => {
    if (!document.body.classList.contains("inbox-popover-open")) return;
    if (rail.contains(event.target) || open.contains(event.target)) return;
    setInboxPopover(false);
  });
  document.addEventListener("keydown", (event) => {
    const inboxVisible = document.body.classList.contains("inbox-popover-open") ||
      (window.innerWidth < 900 && !document.body.classList.contains("inbox-collapsed"));
    if (event.key === "Escape" && !event.defaultPrevented && inboxVisible) {
      dismissInbox({ restoreFocus: true });
    }
  });
  window.addEventListener("resize", () => transitionCleanup?.());
  const views = $("#inbox-views");
  views.onclick = (event) => {
    const button = event.target.closest("[data-inbox-view]");
    if (button) chooseView(button.dataset.inboxView);
  };
  const view = loadRailView(localStorage);
  paintViewSwitch(view);
  setInboxView(view);
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
