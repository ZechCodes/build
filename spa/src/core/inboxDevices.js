// Which device a row in the rail belongs to, and what that means for the row.
//
// The inbox is one list across every machine on the account, so a row names the
// device that answered for it, and a verb on a row must reach THAT device:
// clearing a row the laptop answered for through the desktop's session would
// clear nothing. So every verb asks this module for its call, and a device that
// cannot answer right now hands back one that refuses, at the press, in a
// sentence naming what the press was for — the verb sites are written once, for
// every row, whichever machine it came from. A surface that is about one
// machine rather than one row — the create dialog — asks the same way, by
// device id.
//
// Nothing here shuts a control. What the rail paints is what the cache holds,
// and a machine that is still connecting is painted as if it had answered: its
// rows are greyed and say why, beside everything they held, and every control
// on them stays live. The call behind the control is what refuses.

import { App } from "../app.js";
import { canAnswer, contextFor, creationDevice, homeContext } from "./deviceContexts.js";
import { deviceNameOf } from "./devicePolicy.js";
import { allDevicesOfflineText } from "./text.js";
import { deviceAwayMark, deviceAwayWord } from "./deviceAway.js";
import { deviceOfflineNotice } from "./deviceNotice.js";
import { notifyError } from "./notify.js";
import { EMPTY_CATALOG } from "./modelCatalog.js";

/** What a device that cannot answer offers: nothing to call, the mark a call
 *  that names no verb is refused with, and the word its rows are greyed with —
 *  which say why it cannot, since a machine answering in a shape this tab cannot
 *  read is not away at all. */
const noDevice = (context) => ({ call: null, mark: deviceAwayMark(context), word: deviceAwayWord(context) });

/** The machine a surface is about: the one it names, or the one creation goes
 *  to when it names none. */
const contextOf = (deviceId) => (deviceId ? contextFor(deviceId) : homeContext());

/**
 * What one machine offers right now: `{ call, mark, word }`.
 *
 * Naming no machine means the one where creation goes: home. A row this client
 * is holding itself — a capture taken while no device could take it — names
 * none, and so does a surface that is about nowhere in particular.
 */
function deviceTarget(deviceId) {
  const context = contextOf(deviceId);
  if (!canAnswer(context)) return noDevice(context);
  return { call: context.rpc, mark: null, word: null };
}

/**
 * The harnesses one machine offers, for a surface that starts work there.
 *
 * Asked the same way a call is, so no reader compares device ids or falls back
 * to an account-wide answer: the composer asks for home's, the create dialog
 * and the agent rail for the machine of the address they were given. A machine
 * that cannot answer right now offers what it last said it offered, off disk
 * (Render from cache), and is asked once it can be; one nothing here has held
 * offers the empty catalog — the harness's own default, and nothing to choose
 * between — rather than another machine's list.
 *
 * Asking a machine AGAIN is the settings page's business, not this door's: a
 * page that has just changed what a catalog reports holds that machine's
 * context and tells it directly, so a refused re-read still drops what is now
 * known to be stale.
 */
export const deviceCatalog = (deviceId) => {
  const context = contextOf(deviceId);
  return context ? context.modelCatalog() : Promise.resolve(EMPTY_CATALOG);
};

/** Hear this machine's catalog change: its answer landing after a surface
 *  painted what the disk held, or the settings page asking it again. */
export const followDeviceCatalog = (deviceId, listener) =>
  contextOf(deviceId)?.onModelCatalogChanged(listener) || (() => {});

/** What a press on a machine that cannot answer says, wherever it was
 *  pressed: one plain sentence naming what the press was for. `doing` is that,
 *  in the reader's words — "archive this workspace". */
export const awayRefusal = (doing) => `Build cannot ${doing} because this machine is away.`;

/** The words a call is refused with: the sentence for what it was doing when
 *  its caller said, else the machine's short mark. `doing` is a phrase, or —
 *  for a surface that sends more than one verb through the one call, like a
 *  project's settings sheet — a phrase per method. */
function refusalOf(mark, doing, method, params) {
  if (!doing) return mark;
  return awayRefusal(typeof doing === "function" ? doing(method, params) : doing);
}

/** The call a surface about one machine makes: that machine's, or one that
 *  refuses at the press — so no call site asks whether the machine is there,
 *  and no control has to be shut in case it is not. */
export function deviceCall(deviceId, doing = null) {
  const { call, mark } = deviceTarget(deviceId);
  return call || ((method, params) => Promise.reject(new Error(refusalOf(mark, doing, method, params))));
}

/** The device a row's verbs run against, and the call they make: the row says
 *  which machine answered for it, so no verb site asks whose row it is. */
const verbTarget = (row) => deviceTarget(row && row.deviceId);
export const verbCall = (row, doing = null) => deviceCall(row && row.deviceId, doing);

/**
 * After the paint: grey what no device can answer for right now, and say why
 * on the row.
 *
 * A row's device is its own; a block's is its project's. It is a pass over the
 * painted list rather than a class inside the row renderers because a device
 * goes offline between paints and nothing about a row changes when it does.
 * It only ever marks: every control on a greyed row or block is exactly what it
 * is on a live one, and a press on it is refused by its own call.
 * `lookup`: { entryFor(key), blockFor(projectKey) } — the wiring's own record
 * of what it painted, since a row is never found by a selector built from a key
 * the daemon minted.
 */
export function paintDeviceState(list, { entryFor, blockFor }) {
  for (const element of list.querySelectorAll(".inbox-entry")) {
    markAway(element, greyAway(element, entryFor(element.dataset.key)));
  }
  for (const element of list.querySelectorAll(".inbox-project")) {
    greyAway(element, blockFor(element.dataset.project));
  }
}

/** One row or block: greyed while its own device cannot answer. A row this
 *  client holds itself names no device and is nobody's to grey. Hands back the
 *  word that device's rows wear, which is what a row's own mark is painted
 *  from. */
function greyAway(element, painted) {
  const target = painted && painted.deviceId ? verbTarget(painted) : null;
  const away = Boolean(target && target.mark);
  element.classList.toggle("inbox-offline", away);
  return away ? target.word : null;
}

/** The word a greyed row wears — offline, or the update that would make its
 *  machine readable again. Grey on its own says "this matters less", not "the
 *  machine holding it cannot be asked", so the row says it — on its first line,
 *  where its own tags are, ahead of the unread count. It is put there after the
 *  paint, with the grey, because nothing about a row changes when its machine
 *  goes. */
function markAway(element, away) {
  const shown = element.querySelector(".inbox-away");
  if (!away) {
    shown?.remove();
    return;
  }
  const line = element.querySelector(".inbox-body > .inbox-line");
  if (!line) return;
  const word = shown || line.insertBefore(document.createElement("span"), line.querySelector(".badge"));
  word.className = "dim inbox-away";
  word.textContent = away;
}

/**
 * Where creation goes, in the shape a creation sheet is opened with: that
 * machine's caller and the account's name for it.
 *
 * Null while that machine cannot answer — and the reader is told so under
 * `refusal`, because a control that quietly does nothing is a fault to whoever
 * pressed it. The sheet itself never learns any of this: it is handed one
 * caller and asks nothing about devices. The machine's id rides along for
 * whoever has to stamp it onto what the sheet made, since a fresh creation
 * answer names no machine of its own.
 */
export function creationTarget(refusal) {
  const context = homeContext();
  if (canAnswer(context)) {
    return { callRpc: context.rpc, deviceId: context.deviceId, deviceName: deviceNameOf(App.devices, context.deviceId) };
  }
  notifyError(refusal, creationRefusal());
  return null;
}

/** The reconnect-aware caller for an explicitly chosen creation device. */
export function creationCall(deviceId) {
  const context = contextFor(deviceId);
  if (context?.rpc) return context.rpc;
  return () => Promise.reject(new Error(deviceOfflineNotice(deviceId)));
}

/**
 * Why creation cannot go anywhere right now.
 *
 * The machine it goes to, named: the account knows which one that is long
 * before it can answer — a picked device that is up by the list and still
 * handshaking has no context yet, while every other machine is live, and saying
 * every device is offline there is simply untrue. That sentence is for the case
 * it describes: no machine to name at all.
 */
function creationRefusal() {
  const deviceId = creationDevice();
  return deviceId ? deviceOfflineNotice(deviceId) : allDevicesOfflineText();
}
