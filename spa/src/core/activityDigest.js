/// What a folded run of activity says, and where the number comes from.
///
/// A run's tool-call count is a fact about the whole run, and the whole run is
/// not what a page ships: the bridge caps how many items of any one run travel
/// and sends a DIGEST — the run's span, its exact tool-call count, and its last
/// call — for what it left behind. Counting the rows in hand would say "12"
/// under a thousand calls, and would keep saying a different number as the
/// reader scrolls, so the count is read off the digest and topped up with the
/// calls that arrived after the page was cut.
///
/// Nothing here touches the DOM. The renderer is handed five printed values.

import { EVENT_META } from "./threadEvents.js";

/// The first line of a summary, which is what a row's head shows.
///
/// A row that says only "Agent called a tool" is a row nobody can scan; what
/// the agent actually did is the line under it, and that is the half worth
/// having outside the fold.
export function firstLine(summary) {
  return String(summary || "").split("\n").find((line) => line.trim()) || "";
}

/// Fold the digests a payload carried into the ones already held.
///
/// Keyed by the run's first sequence, so a page re-cut over a run that has
/// since grown replaces what was held for it rather than doubling it, while an
/// older page adds the runs it reached back to. A payload carrying none — every
/// forward delta — leaves the standing answer exactly as it was: a delta says
/// nothing about a run's total, only that more of it arrived.
export function mergeActivityDigests(held = [], payload) {
  const arrived = (payload && payload.activity_digests) || [];
  if (!Array.isArray(arrived) || !arrived.length) return held;
  const byRun = new Map(held.map((digest) => [digest.from_sequence, digest]));
  for (const digest of arrived) byRun.set(digest.from_sequence, digest);
  return [...byRun.values()].sort((a, b) => a.from_sequence - b.from_sequence);
}

/// The digest whose run covers a sequence, or nothing.
///
/// A run is keyed in the timeline by the oldest sequence the WINDOW holds of
/// it, which is not where the run started whenever the page cut one — so what
/// a run's digest is found by is the span it covers, never its first sequence.
export function digestCovering(digests, sequence) {
  return (
    (digests || []).find(
      (digest) => digest.from_sequence <= sequence && digest.through_sequence >= sequence,
    ) || null
  );
}

/// The digest a pressed run must be fetched over, or nothing.
///
/// Two kinds of run are drawn from the window instead. One the page shipped
/// whole — its digest starts no earlier than the oldest row in hand, so there
/// is nothing missing to ask for. And the TAIL run, the one that reaches the
/// end of the conversation: it is still being written, and the window is where
/// its newest rows land. Fetching it would freeze a moving run into a record
/// kept until the entity is evicted, and every call it grew afterwards would
/// fall between that record's end and the window's oldest row, unseen and never
/// asked for again.
///
/// Which run is the tail is answered by everything the client holds about it,
/// never by the digest alone. A digest is cut on a PAGED answer and no forward
/// delta refreshes one, while the conversation's newest sequence moves on every
/// delta — so one call landing on the live tail leaves its digest short of the
/// end while the run is still being written. `runThrough` is the other half of
/// the answer: the newest sequence the window holds for the run, its folded
/// calls included, which is what the run's own box says about itself.
export function runDigestToFetch(digests, runKey, threadLastSequence, runThrough = 0) {
  const key = Number(runKey);
  const digest = digestCovering(digests, key);
  if (!digest || digest.from_sequence >= key) return null;
  const runReaches = Math.max(digest.through_sequence, Number(runThrough) || 0);
  return runReaches >= Number(threadLastSequence) ? null : digest;
}

const sequenceOf = (activity) => (Number.isFinite(activity.sequence) ? activity.sequence : null);

/// The span of the conversation a run in hand covers, or nothing for a run
/// rendered without sequences (the tests, and the initial-message row): with no
/// sequences there is nothing a digest could be matched against.
function heldSpan(activities) {
  const sequences = activities.map(sequenceOf).filter((sequence) => sequence !== null);
  if (!sequences.length) return null;
  return { oldest: Math.min(...sequences), newest: Math.max(...sequences) };
}

/// Every digest whose run overlaps the one in hand. More than one is ordinary:
/// startup rows and hidden messages are filtered out before folding, so a run
/// on this side can be the two the bridge cut either side of what was dropped.
function digestsOver(digests, span) {
  if (!span) return [];
  return digests.filter(
    (digest) => digest.from_sequence <= span.newest && digest.through_sequence >= span.oldest,
  );
}

const newestCovered = (covering) =>
  covering.reduce((newest, digest) => Math.max(newest, digest.through_sequence ?? -Infinity), -Infinity);

const arrivedAfterTheDigest = (toolCalls, coveredThrough) =>
  toolCalls.filter((activity) => {
    const sequence = sequenceOf(activity);
    return sequence === null || sequence > coveredThrough;
  });

/// The newest call any covering digest recorded, shaped like a row in hand.
///
/// A call whose summary the daemon never held has no line to give, so it is no
/// line at all and the run falls back to its latest row — a blank head reads as
/// a bug, where the latest row reads as what the agent is doing.
function digestLastCall(covering) {
  const calls = covering.map((digest) => digest.last_tool_call).filter((call) => call && call.summary);
  if (!calls.length) return null;
  const newest = calls.reduce((latest, call) => (call.sequence > latest.sequence ? call : latest));
  return { meat: firstLine(newest.summary), outcome: newest.outcome, createdAt: newest.created_at };
}

/// The newest call the run holds, by sequence rather than by position.
///
/// A call that spawned a subagent stands for everything that subagent did, so
/// the calls of two subagents working at once interleave: the last call in the
/// list is not the last call the agent made.
function newestHeldCall(toolCalls) {
  if (!toolCalls.length) return null;
  return toolCalls.reduce((latest, call) => (call.sequence > latest.sequence ? call : latest));
}

const lineOf = (source) => ({
  meat: (source && source.meat) || "",
  outcome: source && source.outcome,
  createdAt: (source && source.createdAt) ?? null,
});

/// The four printed values of a head, which are one reading of one thing.
///
/// A call's line wears a call's glyph wherever the line came from — the run's
/// own rows or a digest — because a run that trails off into thinking still
/// says what the agent last DID, and the thought's glyph beside a Bash line
/// reads as the wrong row. A run with no call to name falls back to its latest
/// row whole: that row's words, and that row's glyph.
function headOf(call, latest) {
  if (call) return { icon: EVENT_META.tool_use.icon, ...lineOf(call) };
  return { icon: (latest && latest.icon) || "", ...lineOf(latest) };
}

/// What one folded run prints: how many tools it called, and the last call's
/// line, mark, time and glyph.
///
/// The count is the sum over every digest the run reaches, plus the calls held
/// newer than the newest of them — where a row's calls are the ones it STANDS
/// for: a call that spawned a subagent folds that subagent's calls under it, so
/// a row can be four calls and counting rows would say one.
///
/// A run no digest reaches — one written entirely after the page, or a
/// conversation shipped whole — counts the calls in hand.
/// A run that called no tool at all keeps the old look: its latest line, and how
/// many rows it holds.
export function activityRunSummary(digests, runEntries) {
  const activities = runEntries.map((entry) => entry.activity).filter(Boolean);
  const toolCalls = activities.flatMap((activity) => activity.toolCalls || []);
  const covering = digestsOver(digests || [], heldSpan([...activities, ...toolCalls]));
  const counted = covering.reduce((total, digest) => total + (digest.tool_calls || 0), 0);
  const count = counted + arrivedAfterTheDigest(toolCalls, newestCovered(covering)).length;
  const latest = activities.at(-1);
  const call = count ? newestHeldCall(toolCalls) || digestLastCall(covering) : null;
  return { count: count || runEntries.length, ...headOf(call, latest) };
}
