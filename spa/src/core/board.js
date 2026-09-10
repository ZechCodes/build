// The board's pure model: which run/plan states are terminal, and how a mixed
// list of plans and runs splits into the NEEDS YOU / WORKING / DONE buckets.
// Kept free of DOM/app imports so the bucketing is unit-testable; the board view
// (src/views/board.js) renders these buckets into cards.

/** Terminal run states — the DONE bucket; never running or needs-you. */
export const RUN_TERMINAL_STATES = new Set(["merged", "abandoned", "archived"]);

/** Terminal plan states (abandoned is the only one). */
export const PLAN_TERMINAL_STATES = new Set(["abandoned"]);

/** Split a mixed plan+run board into ordered attn / work / done lists. A terminal
 *  entity lands in `done`, an attention-flagged non-terminal entity in `attn`,
 *  everything else in `work`. Runs come before plans within a bucket. Each entry
 *  is tagged { kind: "run", r } / { kind: "plan", p } so a card renderer can
 *  dispatch by kind and open the matching route. */
export function bucketBoard({ runs = [], plans = [] } = {}) {
  const bucketOf = (needsAttention, terminal) => (terminal ? "done" : needsAttention ? "attn" : "work");
  const byBucket = { attn: [], work: [], done: [] };
  for (const run of runs) byBucket[bucketOf(run.needs_attention, RUN_TERMINAL_STATES.has(run.state))].push({ kind: "run", r: run });
  for (const plan of plans) byBucket[bucketOf(plan.needs_attention, PLAN_TERMINAL_STATES.has(plan.state))].push({ kind: "plan", p: plan });
  return byBucket;
}

/** The project page's attention-ordered grouping, built on bucketBoard. NEEDS
 *  YOU is the attn bucket (in-review plans + attention runs, mixed); READY TO
 *  IMPLEMENT lifts approved plans out of WORKING so the user's next decisive
 *  action isn't buried among still-drafting work; WORKING is the remainder of
 *  the work bucket (running runs + drafting plans); DONE is the terminal bucket.
 *  needsYou/working/done keep bucketBoard's kind-tagged entries; readyPlans is a
 *  plain plan-object list (rendered by the plan-card path). */
export function bucketProjectEntities({ runs = [], plans = [] } = {}) {
  const { attn, work, done } = bucketBoard({ runs, plans });
  const isApprovedPlanEntry = (entry) => entry.kind === "plan" && entry.p.state === "approved";
  const readyPlans = work.filter(isApprovedPlanEntry).map((entry) => entry.p);
  const working = work.filter((entry) => !isApprovedPlanEntry(entry));
  return { needsYou: attn, working, readyPlans, done };
}
