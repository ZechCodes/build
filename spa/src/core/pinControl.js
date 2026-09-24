// One pin, in two heads. The inbox rail (core/inboxShell.js) and the
// conversation panel (core/agentRail.js) offer the same gesture over the same
// pair of states — docked beside the work, or a popover on a strip that goes
// away when you are done with it — so they wear the same icon and say the same
// words about it, from here.
//
// Two entry points because the two heads are written differently: the inbox's
// button stands in index.html and is only ever re-labelled, the panel's head is
// rewritten as markup whenever what it says changes.

import { setAttr } from "../dom.js";
import { ICON_PIN } from "./icons.js";
import { esc, pinText } from "./text.js";

/** The class every pin wears, so one rule dresses both and one selector finds
 *  either. */
export const PIN_CLASS = "pinbtn";

/** The control as markup, for a head that is rewritten rather than wired once. */
export function pinButtonHtml({ subject, pinned }) {
  const label = esc(pinText(pinned, subject));
  return `<button type="button" class="iconbtn ${PIN_CLASS}" aria-pressed="${pinned}"
    title="${label}" aria-label="${label}">${ICON_PIN}</button>`;
}

/** …and the same state written onto a button that is already standing. */
export function syncPinButton(button, { subject, pinned }) {
  const label = pinText(pinned, subject);
  setAttr(button, "aria-pressed", pinned);
  setAttr(button, "aria-label", label);
  setAttr(button, "title", label);
}
