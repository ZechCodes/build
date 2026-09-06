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
/// Nothing here touches the DOM. The renderer is handed four printed values.

const TOOL_CALL_KIND = "tool_use";

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

const lineOf = (source) => ({
  meat: (source && source.meat) || "",
  outcome: source && source.outcome,
  createdAt: (source && source.createdAt) ?? null,
});

/// What one folded run prints: how many tools it called, and the last call's
/// line, mark and time.
///
/// The count is the sum over every digest the run reaches, plus the calls held
/// newer than the newest of them. A run no digest reaches — one written entirely
/// after the page, or a conversation shipped whole — counts the calls in hand.
/// A run that called no tool at all keeps the old look: its latest line, and how
/// many rows it holds.
export function activityRunSummary(digests, runEntries) {
  const activities = runEntries.map((entry) => entry.activity).filter(Boolean);
  const covering = digestsOver(digests || [], heldSpan(activities));
  const toolCalls = activities.filter((activity) => activity.kind === TOOL_CALL_KIND);
  const counted = covering.reduce((total, digest) => total + (digest.tool_calls || 0), 0);
  const count = counted + arrivedAfterTheDigest(toolCalls, newestCovered(covering)).length;
  const latest = activities.at(-1);
  const line = count ? toolCalls.at(-1) || digestLastCall(covering) || latest : latest;
  return { count: count || runEntries.length, ...lineOf(line) };
}
