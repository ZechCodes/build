// The connection icon, at the foot of the conversation rail.
//
// One small ring says the whole of what used to be a stack of toasts in the
// corner: green with the number of machines connected, yellow while one of them
// is being reconnected to — radiating while an attempt is in flight, counting
// down while it waits for the next. It replaces those toasts outright, and says
// one thing they did not: it shows from the FIRST attempt, where they stayed
// quiet until an attempt had already failed. A reconnect is not an error to be
// apologised for after the fact; it is a state the machine is in, and a state
// belongs in a corner of the page rather than over the reader's work.
//
// The toasts that remain elsewhere are the ones a reader has to act on — a
// machine that cannot be reached at all, a refusal with a reason. Those say
// what to do about something; this says what is happening.
//
// What it draws is core/connectionStatusModel.js's answer and nothing else. The
// parts are built once and only their classes and text change, so every move
// between states is a CSS transition rather than a redraw (spa/src/styles.css).

import { App } from "./app.js";
import { esc } from "./core/text.js";
import { deviceRecoverySnapshot, onDeviceRecoveryChanged } from "./connection.js";
import { liveContexts, onDeviceStateChanged } from "./core/deviceContexts.js";
import { connectionStatus } from "./core/connectionStatusModel.js";

/** How often the countdown is redrawn while a machine waits for its next try. */
const TICK_MS = 1000;

const STATE_CLASSES = ["is-connected", "is-attempting", "is-waiting"];

let stopRecoveryWatch = null;
let stopDeviceWatch = null;
let countdownTimer = null;
/** What was last written, so a tick that says nothing writes nothing — a
 *  rewrite mid-transition would cut the transition short. */
let painted = "";
/** The state the ring is showing, which is where its next change animates
 *  FROM, and what a screen reader has already been told. */
let shownState = "";

/** The account's machines, each said to be holding a session or not. "Live"
 *  is `canAnswer` — the same question every surface asks before it calls a
 *  machine — so the number in the ring is the number of machines that would
 *  answer if something asked them now. */
function devicesNow() {
  const live = new Set(liveContexts().map((context) => context.deviceId));
  return App.devices.map((device) => ({ id: device.id, name: device.name, live: live.has(device.id) }));
}

/** The ring's parts, made once and then kept: the button the ring is drawn on,
 *  the radiating circles that run while an attempt is in flight, the one place
 *  a number is written, and the menu behind the press. Everything inside the
 *  button is hidden from a screen reader — the button's own label says all of
 *  it in words, and the menu says it machine by machine. */
function partsOf(host) {
  let button = host.querySelector(".connection-status");
  if (!button) {
    host.innerHTML = `<button type="button" class="connection-status" data-state="" data-from=""
        aria-haspopup="menu" aria-expanded="false"
      ><span class="connection-radiate" aria-hidden="true"></span><span class="connection-centre" aria-hidden="true"></span></button>
      <div class="connection-menu" role="menu" aria-label="Devices" hidden></div>`;
    button = host.querySelector(".connection-status");
    wirePress(host, button);
  }
  return {
    button,
    centre: host.querySelector(".connection-centre"),
    menu: host.querySelector(".connection-menu"),
  };
}

/** One row per machine: what it is called, and what is true of it. Rows are
 *  not pressable — there is nothing to do to a machine from here, and a row
 *  that looks like a button and does nothing is a worse answer than a line of
 *  text. */
const rowHtml = (row) =>
  `<div class="mi" role="menuitem" data-device="${esc(row.id)}" data-status="${esc(row.status)}"><span class="mt">${esc(row.name)}</span><span class="md">${esc(row.label)}</span></div>`;

/** What the menu is showing, so a tick that moves a countdown redraws it and a
 *  repaint that moves nothing leaves it alone. */
const rowsSignature = (rows) => rows.map((row) => `${row.id}:${row.status}:${row.label}`).join("|");

function paintMenu(menu, rows) {
  const signature = rowsSignature(rows);
  if (menu.dataset.rows === signature) return;
  menu.dataset.rows = signature;
  menu.innerHTML = rows.map(rowHtml).join("");
}

// ─── The press ───────────────────────────────────────────────────────────────

/** Whether the menu is showing. Read off the button rather than held beside
 *  it: the button is what a screen reader is told, and two places to ask would
 *  be two answers to keep in step. */
const menuIsOpen = (button) => button.getAttribute("aria-expanded") === "true";

function setMenuOpen(host, open, { restoreFocus = false } = {}) {
  // Asked of the DOM as it stands rather than built: shutting a menu is also
  // what teardown does, and teardown must not stand an icon back up to do it.
  const button = host.querySelector(".connection-status");
  const menu = host.querySelector(".connection-menu");
  if (button) button.setAttribute("aria-expanded", open ? "true" : "false");
  if (menu) menu.hidden = !open;
  host.classList.toggle("is-open", open);
  if (open) {
    document.addEventListener("keydown", onMenuKey, true);
    document.addEventListener("pointerdown", onOutsidePress, true);
    return;
  }
  document.removeEventListener("keydown", onMenuKey, true);
  document.removeEventListener("pointerdown", onOutsidePress, true);
  // Only where the reader shut it themselves: an outside press has already put
  // the focus where they meant it to go.
  if (restoreFocus) button?.focus();
}

function onMenuKey(event) {
  if (event.key !== "Escape") return;
  const host = document.getElementById("connection-status");
  if (!host) return;
  event.preventDefault();
  setMenuOpen(host, false, { restoreFocus: true });
}

function onOutsidePress(event) {
  const host = document.getElementById("connection-status");
  if (!host || host.contains(event.target)) return;
  setMenuOpen(host, false);
}

/** The press itself. A `<button>`, so Enter and Space are the browser's own
 *  and nothing here re-implements them. */
function wirePress(host, button) {
  button.onclick = () => setMenuOpen(host, !menuIsOpen(button), { restoreFocus: true });
}

/** Tell a screen reader what the ring now says — once per state, never once
 *  per second of a countdown. */
function announce(status) {
  const live = document.getElementById("connection-announcement");
  if (!live) return;
  live.textContent = status.visible ? status.label : "";
}

function paint() {
  const host = document.getElementById("connection-status");
  if (!host) return;
  const status = connectionStatus({ devices: devicesNow(), recoveries: deviceRecoverySnapshot() });
  const signature = `${status.visible}|${status.state}|${status.centre}|${status.label}|${rowsSignature(status.rows)}`;
  if (signature === painted) return;
  painted = signature;
  const { button, centre, menu } = partsOf(host);
  if (status.state !== shownState) {
    // Where the change is coming from, for the stylesheet to animate out of.
    button.dataset.from = shownState;
    shownState = status.state;
    announce(status);
  }
  button.dataset.state = status.state;
  button.classList.remove(...STATE_CLASSES);
  button.classList.add(`is-${status.state}`);
  host.hidden = !status.visible;
  button.setAttribute("aria-label", status.label);
  button.setAttribute("title", status.label);
  centre.textContent = status.centre;
  paintMenu(menu, status.rows);
  // A machine that has gone takes its menu with it: a menu standing over an
  // account with nothing in it is a menu about nothing.
  if (!status.visible && menuIsOpen(button)) setMenuOpen(host, false);
  keepTime(status.ticking);
}

/** The countdown's own clock. Only a wait has one: a ring that is not counting
 *  anything has nothing to redraw, and the supervisor speaks for every other
 *  move the ring makes. */
function keepTime(ticking) {
  if (ticking && countdownTimer == null) countdownTimer = setInterval(paint, TICK_MS);
  if (!ticking && countdownTimer != null) {
    clearInterval(countdownTimer);
    countdownTimer = null;
  }
}

export function mountConnectionStatus() {
  unmountConnectionStatus();
  stopRecoveryWatch = onDeviceRecoveryChanged(paint);
  stopDeviceWatch = onDeviceStateChanged(paint);
  paint();
  return unmountConnectionStatus;
}

export function unmountConnectionStatus() {
  stopRecoveryWatch?.();
  stopRecoveryWatch = null;
  stopDeviceWatch?.();
  stopDeviceWatch = null;
  keepTime(false);
  painted = "";
  shownState = "";
  const host = document.getElementById("connection-status");
  if (host) {
    setMenuOpen(host, false);
    host.hidden = true;
    host.innerHTML = "";
    host.classList.remove("is-open");
  }
  const live = document.getElementById("connection-announcement");
  if (live) live.textContent = "";
}
