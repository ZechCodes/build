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
  deviceBridgeBehindText,
  deviceOfflineMark,
  deviceOfflineText,
  deviceOfflineWord,
} from "./text.js";

/** Why a machine cannot answer, by which side is behind. A bridge speaking an
 *  API major nothing here claims is not offline — it is answering, in a shape
 *  this tab cannot read — so everything it is said with names the side that is
 *  behind instead of calling the machine away. */
const UNREACHABLE = {
  app: { sentence: deviceAppBehindText, mark: appBehindMark, word: appBehindWord },
  bridge: { sentence: deviceBridgeBehindText, mark: bridgeBehindMark, word: bridgeBehindWord },
};

/** How a machine that cannot answer is said, by why it cannot. */
const unreachableAs = (context) => UNREACHABLE[context?.unsupported] || null;

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
