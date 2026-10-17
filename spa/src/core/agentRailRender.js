import { QUIET_SHAPE, STARTING_SHAPE, WORKING_SHAPE } from "./agentRailModel.js";
import { esc } from "./text.js";

/** The agent's stable name and its current work occupy separate lines. The
 * work element is absent when no topic exists, and is the only animated node. */
export function railWhoHtml(who, heading) {
  const work = heading?.text || "";
  return `<span class="rail-who" title="${esc(work || who)}"><span class="rail-who-name">${esc(who)}</span>${work
    ? `<span class="rail-who-work">${esc(work)}</span>` : ""}</span>`;
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
