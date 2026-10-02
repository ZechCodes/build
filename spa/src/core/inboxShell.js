// The inbox rail's chrome: whether it is docked or away, which of its two faces
// it is showing, and the account entry at its foot. The entries themselves are
// the inbox view's business; this module only owns the rail as a piece of the
// shell.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { inboxListRouteChanged, mountInboxList, openNewProject, setInboxView } from "./inboxView.js";
import { railViewSwitchHtml } from "./railMode.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { subscribeInboxAttentionCount } from "./inboxAttention.js";
import { ICON_PIN, ICON_PLUS, ICON_SETTINGS } from "./icons.js";
import { anyBridgeUpdateNotifies, onBridgeUpdatesChanged } from "./bridgeUpdates.js";
import { syncPinButton } from "./pinControl.js";
import { mountPushPrompt } from "./pushPrompt.js";
import "../styles/shell.css";

const COLLAPSED_ADDRESS = uiAddress({ view: "inbox", kind: "fold", sub: "rail" });
const VIEW_ADDRESS = uiAddress({ view: "inbox", kind: "filter", sub: "face" });
let collapseRecord = null;
let viewRecord = null;
let pendingCollapseOptions = null;
let desiredCollapsed = false;
let collapsePaint = Promise.resolve();
let railReady = Promise.resolve();

/** The width the rail stops being a column and is laid over the view instead
 *  (styles/shell.css, `@media (max-width: 900px)`). One number, because every
 *  answer that depends on it — whether the pin does anything, whether
 *  navigating puts the rail away, whether the header toggle is still the way
 *  out — has to give the same answer at the same width. */
const RAIL_OVERLAYS_AT = 900;
const railOverlays = () => window.innerWidth <= RAIL_OVERLAYS_AT;

/** Docked or away on this device, given what the user last chose and how much
 *  room there is. A viewport narrow enough to overlay the rail starts with it
 *  away; a choice, once made, is what counts. */
export function railStartsCollapsed(stored, viewportWidth) {
  if (viewportWidth <= RAIL_OVERLAYS_AT) return true;
  if (stored === "1") return true;
  if (stored === "") return false;
  return false;
}

export function setInboxCollapsed(on, { animate = true, persist = true, reveal = on } = {}) {
  desiredCollapsed = on;
  if (persist && collapseRecord) {
    pendingCollapseOptions = { animate, reveal };
    collapsePaint = collapseRecord.write({ collapsed: on });
    return collapsePaint;
  }
  applyInboxCollapsed(on, { animate, reveal });
  collapsePaint = Promise.resolve();
  return collapsePaint;
}

/** Resolves after the last committed rail choice has painted from its cache readback. */
export function whenInboxCollapsedPainted() {
  return collapsePaint;
}

function applyInboxCollapsed(on, { animate, reveal }) {
  const wasCollapsed = document.body.classList.contains("inbox-collapsed");
  setInboxPeek(false);
  if (on) collapsedAt = Date.now();
  const rail = $("#inbox-rail");
  const before = rail?.getBoundingClientRect();
  const foot = rail?.querySelector(".inbox-foot");
  const beforeFootPadding = footerPadding(foot);
  document.body.classList.toggle("inbox-collapsed", on);
  document.body.classList.toggle("inbox-popover-open", on && reveal);
  syncInboxControls(on);
  animateCollapsedChange(animate && wasCollapsed !== on, rail, before, foot, beforeFootPadding);
}

function animateCollapsedChange(changed, rail, before, foot, beforeFootPadding) {
  if (changed) animateInboxTransition(rail, before, foot, beforeFootPadding);
}

let transitionRun = 0;
let transitionCleanup = null;
const LAYOUT_TRANSITION_MS = 240;
const PANEL_TRANSITION_MS = 160;

function footerPadding(foot) {
  return foot ? getComputedStyle(foot).paddingBottom : "";
}

function holdFooterPadding(foot, before, after) {
  if (foot && before !== after) foot.style.paddingBottom = before;
}

function animateFooterPadding(foot, before, after) {
  if (!foot || before === after) return null;
  return foot.animate?.(
    [{ paddingBottom: before }, { paddingBottom: after }],
    { duration: PANEL_TRANSITION_MS, easing: "cubic-bezier(.2,.8,.2,1)" },
  ) || null;
}

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

function animateInboxTransition(rail, before, foot, beforeFootPadding) {
  const run = ++transitionRun;
  transitionCleanup?.();
  transitionCleanup = null;
  if (!rail || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
  const after = rail.getBoundingClientRect();
  const afterFootPadding = footerPadding(foot);
  if (!before?.width || !after.width) return;
  document.body.classList.add("inbox-transitioning");
  pinRailRect(rail, before);
  holdFooterPadding(foot, beforeFootPadding, afterFootPadding);
  let animation = null;
  let footAnimation = null;
  let settleTimer = null;
  const finish = () => {
    if (run !== transitionRun) return;
    clearTimeout(settleTimer);
    footAnimation?.cancel();
    releaseRailRect(rail);
    foot?.style.removeProperty("padding-bottom");
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
    footAnimation = animateFooterPadding(foot, beforeFootPadding, afterFootPadding);
    if (!animation) return finish();
    pinRailRect(rail, after);
    animation.onfinish = finish;
    animation.oncancel = finish;
  }, LAYOUT_TRANSITION_MS);
  transitionCleanup = () => {
    clearTimeout(settleTimer);
    footAnimation?.cancel();
    animation?.cancel();
    releaseRailRect(rail);
    foot?.style.removeProperty("padding-bottom");
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
  // Pinned beside the view the rail is already the inbox, and this toggle only
  // stands on top of it (styles/shell.css hides it there too, so the layout
  // never reserves its room). Pinned OVER the view it is still the way out.
  open.hidden = !collapsed && !railOverlays();
  const count = open.dataset.attentionCount;
  const attention = count ? `, ${count} unread notification${count === "1" ? "" : "s"}` : "";
  const label = `${collapsed ? (visible ? "Close" : "Open") + " the inbox" : "Go to inbox"}${attention}`;
  open.setAttribute("aria-expanded", String(visible));
  open.setAttribute("aria-label", label);
  open.title = label;
}

/** Which of its two shapes the rail is in is a question about the width, and
 *  the width changes with no state of ours changing: a window dragged narrow
 *  leaves the rail pinned and lying OVER the view, where the header toggle is
 *  the way out of it again. So the sync every state change runs is run once
 *  more whenever the width crosses the one number that decides the answer. */
function watchRailShape() {
  const overlaying = window.matchMedia?.(`(max-width: ${RAIL_OVERLAYS_AT}px)`);
  overlaying?.addEventListener?.("change", () => syncInboxControls());
}

/** The thing this pin docks, as the reader would name it. The conversation
 *  panel's pin names its own (core/agentRail.js); the words around both are
 *  core/pinControl.js's. */
const INBOX_SUBJECT = "inbox";

function syncPinControl(pin, collapsed) {
  syncPinButton(pin, { subject: INBOX_SUBJECT, pinned: !collapsed });
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
  const pinnedMobile = railOverlays() && !document.body.classList.contains("inbox-collapsed");
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
    if (accepted && railOverlays()) {
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
  if (viewRecord) void viewRecord.write({ view });
  else { paintViewSwitch(view); setInboxView(view); }
}

let mounted = false;

export function paintBridgeUpdateMark() {
  if (typeof document === "undefined") return;
  const account = $("#nav-account");
  if (!account) return;
  const available = anyBridgeUpdateNotifies(App.devices);
  account.classList.toggle("has-bridge-update", available);
  account.setAttribute("aria-label", available ? "Settings; bridge update available" : "Settings");
  account.querySelector(".bridge-update-dot")?.remove();
  if (available) account.insertAdjacentHTML("beforeend", '<span class="bridge-update-dot" aria-hidden="true"></span>');
}

onBridgeUpdatesChanged(paintBridgeUpdateMark);

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
    return railReady;
  }
  mounted = true;
  const account = $("#nav-account");
  if (account) account.innerHTML = ICON_SETTINGS;
  paintBridgeUpdateMark();
  const startsCollapsed = railStartsCollapsed(null, window.innerWidth);
  setInboxCollapsed(startsCollapsed, { animate: false, persist: false, reveal: false });
  collapseRecord = watchUiState(COLLAPSED_ADDRESS, (saved) => {
    if (typeof saved?.collapsed !== "boolean") return;
    const options = pendingCollapseOptions || { animate: false, reveal: false };
    pendingCollapseOptions = null;
    const collapsed = railStartsCollapsed(saved.collapsed ? "1" : "", window.innerWidth);
    desiredCollapsed = collapsed;
    applyInboxCollapsed(collapsed, options);
  });
  // One toggle in two places: the head button docks or puts the rail away, and
  // the floating one — at the same spot, while the rail is away — docks it.
  const pin = $("#inbox-collapse");
  pin.innerHTML = ICON_PIN;
  pin.onclick = () => {
    if (railOverlays()) return;
    setInboxCollapsed(!desiredCollapsed);
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
      (railOverlays() && !document.body.classList.contains("inbox-collapsed"));
    if (event.key === "Escape" && !event.defaultPrevented && inboxVisible) {
      dismissInbox({ restoreFocus: true });
    }
  });
  window.addEventListener("resize", () => transitionCleanup?.());
  watchRailShape();
  const views = $("#inbox-views");
  views.onclick = (event) => {
    const button = event.target.closest("[data-inbox-view]");
    if (button) chooseView(button.dataset.inboxView);
  };
  paintViewSwitch("inbox");
  setInboxView("inbox");
  viewRecord = watchUiState(VIEW_ADDRESS, (saved) => {
    const view = saved?.view === "projects" ? "projects" : "inbox";
    paintViewSwitch(view);
    setInboxView(view);
  });
  mountInboxList();
  mountPushPrompt($("#push-prompt"));
  inboxRouteChanged();
  railReady = Promise.all([collapseRecord.ready, viewRecord.ready]);
  return railReady;
}

/** Keep the rail tracking the route: the account entry at its foot, and the
 *  entry the route is standing on in the list. */
export function inboxRouteChanged() {
  const account = $("#nav-account");
  if (account) account.classList.toggle("active", App.route.name === "account");
  inboxListRouteChanged();
}
