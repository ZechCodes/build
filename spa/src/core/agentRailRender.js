import { esc } from "./text.js";
import { QUIET_SHAPE, STARTING_SHAPE, WORKING_SHAPE } from "./agentRailModel.js";

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

export function railStatusGitHtml(status) {
  const sync = status.sync ? `<span class="rail-status-sync mono">${esc(status.sync)}</span>` : "";
  const stat = status.stat ? `<span class="rail-status-stat mono">${esc(status.stat)}</span>` : "";
  return sync + stat;
}
