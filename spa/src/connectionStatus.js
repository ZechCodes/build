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

/** The ring's parts, made once and then kept: the radiating circles that run
 *  while an attempt is in flight, and the one place a number is written. Both
 *  are hidden from a screen reader — the ring's label says all of it in
 *  words. */
function partsOf(host) {
  let centre = host.querySelector(".connection-centre");
  if (!centre) {
    host.classList.add("connection-status");
    host.setAttribute("role", "status");
    host.innerHTML = `<span class="connection-radiate" aria-hidden="true"></span><span class="connection-centre" aria-hidden="true"></span>`;
    host.dataset.from = "";
    centre = host.querySelector(".connection-centre");
  }
  return { centre };
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
  const signature = `${status.visible}|${status.state}|${status.centre}|${status.label}`;
  if (signature === painted) return;
  painted = signature;
  const { centre } = partsOf(host);
  if (status.state !== shownState) {
    // Where the change is coming from, for the stylesheet to animate out of.
    host.dataset.from = shownState;
    shownState = status.state;
    announce(status);
  }
  host.dataset.state = status.state;
  host.classList.remove(...STATE_CLASSES);
  host.classList.add(`is-${status.state}`);
  host.hidden = !status.visible;
  host.setAttribute("aria-label", status.label);
  host.setAttribute("title", status.label);
  centre.textContent = status.centre;
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
    host.hidden = true;
    host.innerHTML = "";
    host.dataset.state = "";
    host.dataset.from = "";
  }
  const live = document.getElementById("connection-announcement");
  if (live) live.textContent = "";
}
