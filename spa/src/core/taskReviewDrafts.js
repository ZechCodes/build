// PR request drafts are local interaction state, never replicas or commands.
// Restore their exact request IDs, expected versions and selected sources;
// metadata refresh and hydration never submit a saved request.
import { uiAddress, watchUiState } from "./localUiState.js";

export const reviewCreateDraftAddress = ({ deviceId, projectId, workspaceId }) =>
  uiAddress({ deviceId, entityId: projectId, view: "task-review-create", kind: "draft", sub: workspaceId });

export const reviewActionDraftAddress = ({ deviceId, projectId, taskId, snapshotId = "" }, operation) =>
  uiAddress({ deviceId, entityId: projectId, view: "task-review-action", kind: "draft",
    sub: JSON.stringify([taskId, operation, snapshotId]) });

/** Use schedule for typing and flush before explicit submission or teardown. */
export const watchReviewCreateDraft = (scope, paint, options = {}) =>
  watchUiState(reviewCreateDraftAddress(scope), paint, { debounceMs: 180, ...options });

export const watchReviewActionDraft = (scope, operation, paint, options = {}) =>
  watchUiState(reviewActionDraftAddress(scope, operation), paint, { debounceMs: 180, ...options });
