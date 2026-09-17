import { QUIET_SHAPE, STARTING_SHAPE, WORKING_SHAPE } from "./agentRailModel.js";
import { esc } from "./text.js";

/** The head's name slot: the topic the agent set, or a shimmering "Starting"
 *  until it has. `who` — the topic in full, or the harness while there is no
 *  topic — rides along as the title, so a head too narrow to show the whole
 *  name still gives it up on hover. */
export function railWhoHtml(who, heading) {
  const shown = heading && heading.text ? heading : { text: who, starting: false };
  return `<span class="rail-who${shown.starting ? " rail-who-starting" : ""}" title="${esc(who)}">${esc(shown.text)}</span>`;
}

const LEAD_CLASS = {
  [WORKING_SHAPE]: "rail-status-lead rail-status-working",
  [STARTING_SHAPE]: "rail-status-lead rail-status-starting",
  [QUIET_SHAPE]: "rail-status-lead",
};

export const railStatusLeadClass = (shape) => LEAD_CLASS[shape];

export function railStatusLeadHtml(shape) {
  if (shape === WORKING_SHAPE) {
    return `<span class="rail-status-working-word">Working</span> <span class="rail-status-text"></span>`;
  }
  if (shape === STARTING_SHAPE) return `<span class="rail-status-text"></span>`;
  return "";
}
