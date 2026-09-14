// Which device a row in the rail belongs to, and what that means for the row.
//
// The inbox is one list across every machine on the account, so a row names the
// device that answered for it, and a verb on a row must reach THAT device:
// clearing a row the laptop answered for through the desktop's session would
// clear nothing. So every verb asks this module for its call, and a device that
// cannot answer right now hands back one that refuses in the same words the row
// is greyed with — the verb sites are written once, for every row, whichever
// machine it came from. A surface that is about one machine rather than one row
// — the create dialog — asks the same way, by device id.

import { contextFor, homeContext } from "./deviceContexts.js";

/** What a device that cannot answer offers: nothing to call, and the words its
 *  rows and their menus are titled with. */
const NO_DEVICE = Object.freeze({ call: null, disabled: "Device offline" });

/**
 * What one machine offers right now: `{ call, disabled }`.
 *
 * Naming no machine means the one where creation goes: home. A row this client
 * is holding itself — a capture taken while no device could take it — names
 * none, and so does a surface that is about nowhere in particular.
 */
export function deviceTarget(deviceId) {
  const context = deviceId ? contextFor(deviceId) : homeContext();
  if (!context || context.offline || !context.call) return NO_DEVICE;
  return { call: context.call, disabled: false };
}

/** The call a surface about one machine makes: that machine's, or one that
 *  refuses in the words its rows are greyed with — so no call site asks whether
 *  the machine is there. */
export function deviceCall(deviceId) {
  const { call, disabled } = deviceTarget(deviceId);
  return call || (() => Promise.reject(new Error(disabled)));
}

/** The device a row's verbs run against, and the call they make: the row says
 *  which machine answered for it, so no verb site asks whose row it is. */
export const verbTarget = (row) => deviceTarget(row && row.deviceId);
export const verbCall = (row) => deviceCall(row && row.deviceId);

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
