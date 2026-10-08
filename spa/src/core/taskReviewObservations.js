// Observation revisions belong to one directory, independently of the review
// version. Replace a whole fact so recovered reads cannot retain old errors.
const revisionOf = (observation) => Number(observation?.revision) || 0;
const newerObservation = (held, next, heldOrder, readOrder) => !held ||
  revisionOf(next) > revisionOf(held) ||
  (revisionOf(next) === revisionOf(held) && readOrder >= heldOrder);

/** Called inside the review record's atomic merge, so simultaneous replies
 * from other tabs compare the facts that actually committed. Missing entries
 * retain their last observation; only an authoritative missing review clears. */
export function mergeReviewObservations(held, incoming, readOrder) {
  if (!Array.isArray(incoming)) return null;
  const observations = new Map((held?.sync || []).map((fact) => [fact.directory_id, fact]));
  const orders = { ...held?.sync_read_orders };
  let changed = !Array.isArray(held?.sync);
  for (const fact of incoming) {
    const key = fact.directory_id;
    if (!newerObservation(observations.get(key), fact, orders[key] || 0, readOrder)) continue;
    observations.set(key, fact);
    orders[key] = readOrder;
    changed = true;
  }
  return changed ? { sync: [...observations.values()], sync_read_orders: orders } : null;
}
