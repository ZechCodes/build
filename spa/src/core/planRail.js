// Pure bucketing for the project page's Plans rail. Kept free of any app-shell
// import so it stays a unit-testable string/data helper (the way the other
// core/* presentation helpers are), independent of the live App object.

/** Which Plans-rail bucket a plan belongs to. Draft holds a plan still being
 *  authored (created/drafting); In review holds a plan awaiting the user — the
 *  plan_review gate and every parked arm (blocked/failed/idle_unreported/
 *  interrupted), all of which need a human to move forward; Approved holds an
 *  approved plan ready to Implement; History holds the terminal (abandoned)
 *  plans. */
export function planBucketKey(state) {
  if (state === "abandoned") return "history";
  if (state === "approved") return "approved";
  if (state === "created" || state === "drafting") return "draft";
  return "review"; // plan_review + blocked/failed/idle_unreported/interrupted
}

/** Split a plan list into the rail's ordered buckets (draft → review →
 *  approved → history). */
export function bucketPlans(plans) {
  const buckets = { draft: [], review: [], approved: [], history: [] };
  for (const plan of plans || []) buckets[planBucketKey(plan.state)].push(plan);
  return buckets;
}
