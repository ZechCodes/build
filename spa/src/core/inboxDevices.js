// Which device a row in the rail belongs to, and what that means for the row.
//
// The inbox is one list across every machine on the account, so a row names the
// device that answered for it. Two things follow, and both live here.
//
// A verb on a row must reach THAT device: clearing a row the laptop answered
// for through the desktop's session would clear nothing. So every verb asks
// this module for its call, and a device that cannot answer right now hands
// back one that refuses in the same words the row is greyed with — the verb
// sites are written once, for every row, whichever machine it came from.
//
// And until a route can name a device (stage 2), a row on any device but the
// home one has nowhere to open: `#/project/proj-1` would land on whichever
// machine the page is pointed at, and every machine mints a `proj-1`. Such a
// row still works — it just does not open, and it says why.

import { contextFor, homeContext } from "./deviceContexts.js";

/** What a device that cannot answer offers: nothing to call, and the words its
 *  rows and their menus are titled with. */
const NO_DEVICE = Object.freeze({ call: null, disabled: "Device offline" });

/**
 * The device a row's verbs run against: `{ call, disabled }`.
 *
 * A row this client is holding itself — a capture taken while no device could
 * take it — names no device, and belongs where creation goes: home.
 */
export function verbTarget(row) {
  const context = row && row.deviceId ? contextFor(row.deviceId) : homeContext();
  if (!context || context.offline || !context.call) return NO_DEVICE;
  return { call: context.call, disabled: false };
}

/** The call a row's verbs make. A row whose device cannot answer gets one that
 *  refuses, saying what the row itself says, so no verb site asks whose row it
 *  is or whether the machine is there. */
export function verbCall(row) {
  const { call, disabled } = verbTarget(row);
  return call || (() => Promise.reject(new Error(disabled)));
}

/** What a row on another device is titled with instead of what it opens. */
export const AWAY_TITLE = "Opens once this page can name its device";

/** Stage 1 only. A row or a block on any device but the home one opens
 *  nowhere; stage 2 gives routes a device and deletes this. A row this client
 *  holds itself names no device and is the home device's to open. */
export function openableHere(row) {
  if (!row.deviceId || row.deviceId === homeContext()?.deviceId) return row;
  return { ...row, route: null, title: AWAY_TITLE };
}

export const openableHereRows = (rows) => rows.map(openableHere);

/** A project block and everything under it, likewise: a block on another device
 *  opens no checkout, and neither do its rows or its Recent. */
export const openableHereBlock = (block) => ({
  ...openableHere(block),
  entries: openableHereRows(block.entries),
  recent: openableHereRows(block.recent),
});

/**
 * After the paint: grey what no device can answer for right now, and shut the
 * controls that would have asked.
 *
 * A row's device is its own; a block's is its project's. It is a pass over the
 * painted list rather than a class inside the row renderers because a device
 * goes offline between paints and nothing about a row changes when it does.
 * `lookup`: { entryFor(key), blockFor(projectKey) } — the wiring's own record
 * of what it painted, since a row is never found by a selector built from a key
 * the daemon minted.
 */
export function paintDeviceState(list, { entryFor, blockFor }) {
  for (const element of list.querySelectorAll(".inbox-entry")) {
    markDeviceState(element, entryFor(element.dataset.key), ".inbox-menu .mi");
  }
  for (const element of list.querySelectorAll(".inbox-project")) {
    markDeviceState(element, blockFor(element.dataset.project), ":scope > .inbox-project-head .inbox-project-create");
  }
}

/** One row or block: greyed while its own device is away, and every control
 *  named by `controls` shut with the reason. A row this client holds itself
 *  names no device and is nobody's to grey. */
function markDeviceState(element, painted, controls) {
  const reason = painted && painted.deviceId ? verbTarget(painted).disabled : false;
  element.classList.toggle("inbox-offline", Boolean(reason));
  for (const control of element.querySelectorAll(controls)) {
    if (reason) shutControl(control, reason);
    else openControl(control);
  }
}

/** A control whose device cannot answer: it says so, and it does nothing. What
 *  it says when it works is kept, so it can say it again. */
function shutControl(control, reason) {
  if (!("deviceTitle" in control.dataset)) control.dataset.deviceTitle = control.title;
  control.title = reason;
  control.setAttribute("aria-disabled", "true");
  control.setAttribute("disabled", "");
}

function openControl(control) {
  if (!("deviceTitle" in control.dataset)) return;
  control.title = control.dataset.deviceTitle;
  delete control.dataset.deviceTitle;
  control.removeAttribute("aria-disabled");
  control.removeAttribute("disabled");
}
