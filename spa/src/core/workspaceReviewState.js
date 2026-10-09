// Workspace review surfaces read replicas only. F owns review resolution,
// version ordering and capability semantics; this module names the UI facts.
import { readCached } from "./localCache.js";
import { readWorkspaceReview } from "./taskReviewCache.js";
import { readReviewSupport, reviewSupportFor } from "./taskReviewSupport.js";
import { cachedFeedView } from "./cachedRows.js";
import { directoryCacheId } from "./directoryScope.js";
import { workspaceScope } from "./workspaceModel.js";
import { deviceKey } from "./deviceKey.js";
import { assigneeOptions, workspaceAgents } from "./trackerAssignee.js";
import { readTaskRecord } from "./trackerCache.js";
import { timelineRows } from "./trackerTimeline.js";
import { readUiRecord } from "./localUiStore.js";
import { reviewCreateDraftAddress } from "./taskReviewDrafts.js";

export const workspaceReviewListAddress = ({ deviceId }) => ({ deviceId, entityId: "", kind: "workspaces" });
export const shortReviewRef = (ref = "") => ref.replace(/^refs\/heads\//, "");
export const reviewBranchPreview = (title) => `review/<number>-${title.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 50).replace(/-$/, "") || "task"}`;

async function sourceRecords(scope, workspace) {
  return Promise.all((workspace?.directories || []).map(async (directory) => {
    const gitScope = workspaceScope(scope.workspaceId, directory.source_id || directory.id, workspace);
    const entityId = directoryCacheId(gitScope);
    const address = (kind) => ({ deviceId: scope.deviceId, entityId, kind });
    const [refs, status] = await Promise.all([readCached(address("refs")), readCached(address("status"))]);
    return { directory, gitScope, entityId, refs: refs?.value, status: status?.value };
  }));
}

export async function readWorkspaceReviewContext(scope) {
  const [feed, support, held] = await Promise.all([
    cachedFeedView(scope.deviceId), readReviewSupport(scope.deviceId), readWorkspaceReview(scope),
  ]);
  const workspace = feed.workspaces.find((row) => (row.workspace_id || row.id) === scope.workspaceId);
  const project = feed.projects.find((row) => row.id === scope.projectId);
  const projectKey = deviceKey(scope.deviceId, scope.projectId);
  const keyed = { ...feed,
    workspaces: feed.workspaces.map((row) => ({ ...row, projectKey: deviceKey(scope.deviceId, row.project_id) })),
    items: feed.items.map((row) => ({ ...row, projectKey: deviceKey(scope.deviceId, row.project_id) })),
  };
  const catalog = (await readCached({ deviceId: scope.deviceId, entityId: "", kind: "models" }))?.value;
  const task = held?.review?.task_id ? await readTaskRecord(scope.deviceId, scope.projectId, held.review.task_id) : null;
  const createDraft = (await readUiRecord(reviewCreateDraftAddress(scope)))?.value;
  return { workspace, project, held, createDraft, reviewedSnapshot: lastReviewedSnapshot(held?.review, task?.timeline), sources: await sourceRecords(scope, workspace), catalog,
    reviewerOptions: assigneeOptions(workspaceAgents(keyed, projectKey)).filter((option) => ["unassign", "user", "project_agent", "agent"].includes(option.kind)),
    support: reviewSupportFor(held?.review, support) };
}

export function boundSourceFacts(context) {
  const held = context.held;
  return (held?.review?.bindings || []).map((binding) => {
    const source = context.sources.find(({ directory }) => directory.id === binding.directory_id);
    const sync = held.sync?.find((fact) => fact.directory_id === binding.directory_id);
    const differentBranch = sourceOnDifferentBranch(source, binding);
    return { binding, source, sync, differentBranch, sinceReview: sourceSinceReview(context, binding, sync) };
  });
}

function sourceOnDifferentBranch(source, binding) {
  const current = source?.refs?.current;
  if (current?.kind && current.kind !== "branch") return true;
  const branch = current?.full_ref || source?.directory.branch;
  return Boolean(branch) && shortReviewRef(branch) !== shortReviewRef(binding.dedicated_branch_ref);
}

function lastReviewedSnapshot(review, timeline) {
  const opinion = timelineRows(timeline).filter((row) => row.actor?.kind === "user" && row.opinion?.snapshot_id).at(-1)?.opinion;
  return review?.snapshots?.find((snapshot) => snapshot.id === opinion?.snapshot_id);
}

export function formatReviewSinceNote({ count, head, reviewedHead, rewritten = false }) {
  if (Number.isInteger(count)) return count ? `${count} commit${count === 1 ? "" : "s"} since your last review` : "";
  if (!reviewedHead) return "";
  if (rewritten) return "History rewritten since your last review";
  return head && head !== reviewedHead ? "Changed since your last review" : "";
}

const directoryHead = (snapshot, binding) => snapshot?.directories.find((directory) => directory.id === binding.directory_id)?.head;

function publishedRewriteSince(snapshots, baseline, binding) {
  return snapshots.filter((snapshot) => snapshot.number > baseline.number)
    .some((snapshot) => snapshot.publication?.directories.some((directory) => directory.directory_id === binding.directory_id && directory.rewritten));
}

// The bridge's exact count only answers for the baseline it was taken against.
const countedSince = (sync, baseline) => (sync?.reviewed_snapshot_id === baseline.id ? sync : {});

function sourceSinceReview(context, binding, sync) {
  const snapshots = context.held.review.snapshots;
  const baseline = context.reviewedSnapshot;
  const head = directoryHead(snapshots.at(-1), binding) || sync?.snapshot_head;
  if (!baseline) return formatReviewSinceNote({ head });
  const counted = countedSince(sync, baseline);
  const rewritten = Boolean(counted.rewritten_since_review) || publishedRewriteSince(snapshots, baseline, binding);
  return formatReviewSinceNote({ count: counted.commits_since_review, head, reviewedHead: directoryHead(baseline, binding), rewritten });
}

const rewrittenText = "Review history was rewritten or diverged";
const rewrittenObservation = (sync) => sync.rewritten || sync.diverged;
function observationPendingText(sync) {
  if (sync.error) return sync.error;
  if (Number.isInteger(sync.pending_commits)) return `${sync.pending_commits} commit${sync.pending_commits === 1 ? "" : "s"} not pushed to review`;
  if (rewrittenObservation(sync)) return rewrittenText;
  return "Pending commits unavailable";
}
export function pendingReviewText({ sync, binding }) {
  if (sync) return observationPendingText(sync);
  return binding.publication === "published" ? "0 commits not pushed to review" : "Pending commits unavailable";
}

export const canPushSource = (fact) => Boolean(fact) && !fact.differentBranch && Boolean(fact.source) &&
  Boolean(fact.sync?.working_head && fact.sync?.received_head) && !fact.sync.error &&
  ["pending", "current"].includes(fact.sync.health);

export const pushSourceParams = ({ binding, sync }) => ({ directory_id: binding.directory_id,
  expected_head: sync.working_head, expected_received_head: sync.received_head });

export const sourcePushDestination = ({ binding }) =>
  `${binding.remote_name} ${binding.dedicated_branch_ref}:${binding.receiving_ref}`;
