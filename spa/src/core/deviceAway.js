// Why a machine cannot answer, in the three lengths the app has room for: the
// sentence a surface stands up in place of a frame, the mark a shut control —
// or a refused call — wears, and the one word a greyed row carries.
//
// Kept apart from the surfaces that print it because the registry itself reads
// it: a caller to a machine that cannot answer is refused in these same words
// (core/deviceContexts.js), so the refusal and the strip over the surface it
// refused say one thing.

import {
  appBehindMark,
  appBehindWord,
  bridgeBehindMark,
  bridgeBehindWord,
  deviceAppBehindText,
  deviceBlockedMark,
  deviceBlockedText,
  deviceBlockedWord,
  deviceBridgeBehindText,
  deviceOfflineMark,
  deviceOfflineText,
  deviceOfflineWord,
} from "./text.js";

/**
 * Why a direct connection to a machine could not be made, in plain words: one
 * phrase per reason the connect sequence can fail with (spec rule 3).
 *
 * The reasons are the vocabulary, and this is the only place they are spelled:
 * `connection.js` names one when it blocks a device, the context wears it, and
 * every surface over that machine reads it back out through the three lengths
 * below.
 */
const BLOCKED_WHY = {
  "no-webrtc": "this browser cannot open one",
  "ice-servers": "the connection servers could not be reached",
  refused: "the machine would not take the offer",
  timeout: "it did not open in time",
  failed: "it failed",
  lost: "it dropped, and could not be made again",
  unreached: "the machine is not answering",
};

/**
 * The offline mark a blocked machine's context wears, for `setContextOffline`.
 *
 * Not `deviceAwayMark` below, which is a label a control wears: this is the
 * record — away, and why — that the registry writes on the machine, and what
 * every reading of the three lengths is derived from.
 */
export const blockedMark = (reason) => ({ offline: true, blocked: reason });

/** The one sentence about a machine whose direct connection could not be made,
 *  in the account's name for it when the caller has one. */
export const blockedText = (reason, name = null) =>
  deviceBlockedText(name, BLOCKED_WHY[reason] || BLOCKED_WHY.failed);

/** Why a machine cannot answer, by which side is behind. A bridge speaking an
 *  API major nothing here claims is not offline — it is answering, in a shape
 *  this tab cannot read — so everything it is said with names the side that is
 *  behind instead of calling the machine away. */
const UNREACHABLE = {
  app: { sentence: deviceAppBehindText, mark: appBehindMark, word: appBehindWord },
  bridge: { sentence: deviceBridgeBehindText, mark: bridgeBehindMark, word: bridgeBehindWord },
};

/** Every blocked reason said the three ways, from the one table of phrases. */
const BLOCKED = Object.fromEntries(
  Object.keys(BLOCKED_WHY).map((reason) => [
    reason,
    { sentence: (name) => blockedText(reason, name), mark: deviceBlockedMark, word: deviceBlockedWord },
  ]),
);

/** How a machine that cannot answer is said, by why it cannot. A machine this
 *  browser could not reach directly is said in its own words — it is not
 *  offline and it is not behind, it is blocked (rule 3) — and that is the
 *  newest news about it, so it is read first. */
const unreachableAs = (context) => {
  if (context?.blocked) return BLOCKED[context.blocked] || BLOCKED.failed;
  return UNREACHABLE[context?.unsupported] || null;
};

/** The one sentence about a machine that cannot answer, in the account's name
 *  for it. */
export function deviceAwayText(context, name) {
  const said = unreachableAs(context);
  return said ? said.sentence(name, context.apiVersion) : deviceOfflineText(name);
}

/** The short mark a control shut for want of a machine wears: its title, and
 *  the words a call to that machine is refused with. */
export const deviceAwayMark = (context) => unreachableAs(context)?.mark || deviceOfflineMark;

/** The one word a greyed row wears to say why it is grey. */
export const deviceAwayWord = (context) => unreachableAs(context)?.word || deviceOfflineWord;
