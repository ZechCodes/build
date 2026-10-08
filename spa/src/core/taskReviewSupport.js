// Greetings write capabilities once; review surfaces paint this cached fact.
import { mergeCachedAtomically, readCached } from "./localCache.js";
import { ApiError, REVIEW_METHODS } from "./bridgeApi/v1/index.js";

export const PR_REVIEW_VERBS = Object.freeze(["open", "push", "update", "merge", "close", "reopen", "refresh"]);
const snapshotMutations = new Set(["snapshot", "act", "complete"]);
const verbsByMethod = new Map(Object.entries(REVIEW_METHODS).map(([verb, method]) => [method, verb]));

export const NO_REVIEW_SUPPORT = Object.freeze({
  get: false, snapshot: false, diff: false, complete: false, act: false, comments: false,
  pullRequests: false, open: false, push: false, update: false, merge: false, close: false, reopen: false, refresh: false,
});
export const reviewSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "task-review-support" });

const normalizedSupport = (support) => Object.fromEntries(Object.keys(NO_REVIEW_SUPPORT)
  .map((key) => [key, support?.[key] === true]));

export function rememberReviewSupport(deviceId, capabilities, current = () => true) {
  const next = normalizedSupport(capabilities?.reviews);
  return mergeCachedAtomically(reviewSupportAddress(deviceId), (held) =>
    !current() || JSON.stringify(held) === JSON.stringify(next) ? null : next);
}

export async function readReviewSupport(deviceId) {
  return normalizedSupport((await readCached(reviewSupportAddress(deviceId)))?.value);
}

/** Raw greeting flags remain independent; a PR operation needs both its verb
 * and the PR feature. Saved PRs never offer Snapshot-mode mutations. */
export function reviewSupportFor(review, support = NO_REVIEW_SUPPORT) {
  const next = normalizedSupport(support);
  for (const verb of PR_REVIEW_VERBS) next[verb] = next.pullRequests && next[verb];
  if (review?.mode === "pull_request") {
    for (const verb of snapshotMutations) next[verb] = false;
  }
  return next;
}

export function canReviewOperation(support, verb, review = null) {
  return Object.hasOwn(REVIEW_METHODS, verb) && reviewSupportFor(review, support)[verb] === true;
}

const unsupportedOperation = () => new ApiError("unknown_method", "Update this device's bridge to use this review operation.");

/** A cache-backed surface may hold an older greeting. Recheck the session's
 * compatible greeting at dispatch, passing the wire shape and result intact.
 * Load the dispatcher only when called so cache readers stay transport-free. */
export function reviewRpc(context, callRpc = (...asked) => context.rpc(...asked)) {
  return async (...asked) => {
    const verb = verbsByMethod.get(asked[0]);
    if (!verb) throw unsupportedOperation();
    const { whenGreeted } = await import("./deviceContexts.js");
    const request = await whenGreeted(context, () => {
      if (!canReviewOperation(context?.adapter?.capabilities?.reviews, verb)) throw unsupportedOperation();
      return callRpc(...asked);
    });
    if (!request) throw new Error("This machine is unavailable.");
    return request.sent;
  };
}

/** Production repositories have a registered device. Standalone injected
 * callers do not; keep that adapter seam while guarding real mutations on the
 * latest greeting. The original caller retains its captured request priority. */
export function reviewMutationRpc(deviceId, callRpc) {
  return async (...asked) => {
    const { contextFor } = await import("./deviceContexts.js");
    const context = contextFor(deviceId);
    return context ? reviewRpc(context, callRpc)(...asked) : callRpc(...asked);
  };
}
