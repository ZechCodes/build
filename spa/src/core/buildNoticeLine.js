// What Build's own notices say when they are one line.
//
// Zech: "Agents receive 3 kinds of messages: from the user, from other agents,
// and notifications … notifications are a single line left aligned."
//
// A notification is written for the AGENT: the restart notice tells it to
// carry on and what not to trust, the reminder lists every issue it still
// holds. All of that has to reach the agent, and none of it has to be on
// screen — the reader wants to know one arrived, not to read the instructions
// somebody else was given. So the body is kept, in full, behind a press, and
// the line is a summary of it.
//
// The summaries are derived from the body rather than from a field, because
// there is no field: Build writes these as prose. Every one of them therefore
// falls back to the body's own first sentence, which is always a true summary
// even when the shape is one this build has never seen.
//
// Pure: no DOM, no app imports.

import { clockTime } from "./text.js";

/** The first sentence of a body, which is what every notice leads with and is
 *  the honest fallback for one whose shape is unrecognised. */
function firstSentence(body) {
  const line = String(body || "").split("\n").find((one) => one.trim()) || "";
  const stop = line.indexOf(". ");
  return (stop === -1 ? line : line.slice(0, stop + 1)).trim();
}

/** "Build restarted at 4:42 pm and brought this session back." The stamp in
 *  the body is an ISO instant; a reader wants a clock. */
function restartSummary(body) {
  const stamp = String(body || "").match(/restarted at (\S+)/);
  const at = stamp ? Date.parse(stamp[1]) : NaN;
  if (!Number.isFinite(at)) return "Build restarted and brought this session back";
  return `Build restarted at ${clockTime(at)} and brought this session back`;
}

/** "Build: 9 issues still held — #38, #34, #33 and 6 more." The list is the
 *  news; the instructions under it are for the agent. */
function heldSummary(body) {
  const numbers = [...String(body || "").matchAll(/^-\s*#(\d+)/gm)].map((one) => `#${one[1]}`);
  if (!numbers.length) return firstSentence(body);
  const shown = numbers.slice(0, 3).join(", ");
  const rest = numbers.length - 3;
  const said = rest > 0 ? `${shown} and ${rest} more` : shown;
  return `Build: ${numbers.length} issue${numbers.length === 1 ? "" : "s"} still held — ${said}`;
}

/**
 * One line for one of Build's notices.
 *
 * Recognised by what the body says, because that is all there is to go on.
 * Anything unrecognised is its own first sentence — never blank, and never the
 * whole of a page of instructions.
 */
export function buildNoticeSummary(message) {
  const body = String(message?.body || "");
  if (/^Build restarted at /.test(body)) return restartSummary(body);
  if (/issues? assigned to you (are|is) still open/.test(body)) return heldSummary(body);
  return firstSentence(body) || "Build sent a notice";
}

/** Whether there is more to the notice than its line says, which is what
 *  decides whether the row offers a press at all. */
export const noticeHasMore = (message, summary) => {
  const body = String(message?.body || "").trim();
  return Boolean(body) && body !== summary;
};
