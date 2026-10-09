// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { readUiRecord, wipeUiRecords, writeUiRecord } from "../src/core/localUiStore.js";
import { reviewActionDraftAddress } from "../src/core/taskReviewDrafts.js";
import { mountTaskReviewMerge, settledMerge } from "../src/core/taskReviewMerge.js";

const scope = { deviceId: "merge-device", projectId: "proj-1", taskId: "task-1", snapshotId: "snapshot-1" };
const oldHead = "1".repeat(40);
const savedHead = "2".repeat(40);
const mergedHead = "3".repeat(40);
const directory = (id) => ({ id, name: id, status: "git", head: savedHead });
const snapshot = { id: scope.snapshotId, number: 1, directories: [directory("api"), directory("ui"), { id: "live", status: "not_git" }] };
const binding = (id) => ({ directory_id: id, base_branch_ref: "refs/heads/main", source_repository: `/sources/${id}`,
  repository_id: `repo-${id}`, remote_name: "build-review" });
const destination = (id) => ({ directory_id: id, snapshot_id: snapshot.id, source_path: `/sources/${id}`,
  remotes: [{ name: "origin", branches: ["main", "release"] }] });
const review = { task_id: scope.taskId, version: 3, mode: "pull_request", state: "open", snapshots: [snapshot],
  pull_request: { status: "approved", latest_published_snapshot_id: snapshot.id },
  bindings: [binding("api"), binding("ui")], destinations: [destination("api"), destination("ui")], actions: [] };
const record = { review, sync: ["api", "ui"].map((id) => ({ directory_id: id, health: "current", revision: 1,
  target_head: oldHead, received_head: savedHead })), merge_intents: [] };
const source = (id, push = true) => ({ directory_id: id, expected_base_head: oldHead, ...(push ? { push: { remote: "origin", branch: "main" } } : {}) });
const params = { expected_version: 3, snapshot_id: snapshot.id, sources: [source("api"), source("ui")] };
const intent = (state = "failed") => ({ request_id: "merge-1", state, request: { ...params, sources: params.sources.map((value) => ({ ...value,
  head: savedHead, base_branch_ref: "refs/heads/main", repository_id: `repo-${value.directory_id}` })) }, action_ids: ["action-api", "action-ui"] });
const action = (id, mergeStatus = "succeeded", pushStatus = "failed") => ({ id: `action-${id}`, directory_id: id, snapshot_id: snapshot.id,
  source_name: id, source_path: `/sources/${id}`, status: pushStatus === "succeeded" ? "succeeded" : "failed", steps: [
    { kind: "merge", branch: "main", status: mergeStatus, input_head: savedHead, result_head: mergeStatus === "succeeded" ? mergedHead : undefined },
    { kind: "push", branch: "main", remote: "origin", status: pushStatus, input_head: mergedHead,
      result_head: pushStatus === "succeeded" ? mergedHead : undefined, error: pushStatus === "failed" ? "Publication refused" : undefined },
  ] });
let panel;
const mount = async (next = record, repository = { mutate: vi.fn() }, support = { pullRequests: true, merge: true }) => {
  document.body.innerHTML = '<div id="merge"></div>';
  panel = mountTaskReviewMerge(document.querySelector("#merge"), { ...scope, snapshot, review: next.review, record: next, support, repository });
  await panel.ready;
  return repository;
};
const field = (kind, id) => document.querySelector(`[data-pr-merge-${kind}="${id}"]`);
const change = (kind, id, value) => {
  const control = field(kind, id);
  if (control.type === "checkbox") control.checked = value; else control.value = value;
  control.dispatchEvent(new Event(kind === "branch" ? "input" : "change"));
};
const submit = () => document.querySelector("[data-pr-merge-form]").onsubmit({ preventDefault() {} });
beforeEach(async () => { panel?.dispose(); await wipeUiRecords(); });
afterEach(() => panel?.dispose());

it("merges the fixed bindings into configured bases with current sync target preconditions and optional Push", async () => {
  const repository = await mount();
  expect(document.querySelectorAll("[data-pr-merge-source]")).toHaveLength(2);
  expect(document.body.textContent).toContain("main");
  change("push", "api", true);
  change("branch", "api", "release/new");
  panel.update({ ...review, version: 4 }, record);
  await submit();
  expect(repository.mutate).toHaveBeenCalledExactlyOnceWith("merge", { expected_version: 4, snapshot_id: snapshot.id,
    sources: [{ ...source("api"), push: { remote: "origin", branch: "release/new" } }, source("ui", false)] });
  expect((await readUiRecord(reviewActionDraftAddress(scope, "merge"))).value.submitted.expected_version).toBe(4);
});

it("requires exact Merge support and PR support while retaining cached failure results", async () => {
  const failed = { ...record, review: { ...review, actions: [action("api")] }, merge_intents: [intent()] };
  for (const support of [{ merge: true, pullRequests: false }, { merge: false, pullRequests: true, act: true, complete: true }]) {
    panel?.dispose();
    const repository = await mount(failed, { mutate: vi.fn() }, support);
    expect(document.querySelector("[data-pr-merge-form]")).toBeNull();
    expect(document.querySelector("[data-pr-merge-retry]")).toBeNull();
    expect(document.body.textContent).toContain("Publication refused");
    expect(repository.mutate).not.toHaveBeenCalled();
  }
});

it("refuses a historical snapshot, missing target head, moved receiving head and invalid Push branch", async () => {
  for (const altered of [
    { ...record, review: { ...review, pull_request: { ...review.pull_request, latest_published_snapshot_id: "new-look" } } },
    { ...record, sync: record.sync.map((row) => ({ ...row, target_head: undefined })) },
    { ...record, sync: record.sync.map((row) => ({ ...row, received_head: mergedHead })) },
  ]) {
    panel?.dispose();
    const repository = await mount(altered);
    expect(document.querySelector("[data-pr-merge-submit]")?.disabled).toBe(true);
    await submit();
    expect(repository.mutate).not.toHaveBeenCalled();
  }
  panel.dispose();
  const repository = await mount();
  change("push", "api", true); change("branch", "api", "bad..branch");
  await submit();
  expect(repository.mutate).not.toHaveBeenCalled();
  expect(document.querySelector("[data-pr-merge-error]").textContent).toContain("valid");
});

it("paints authoritative cache results only and ignores a successful RPC answer until cache update", async () => {
  const merged = { ...review, version: 10, pull_request: { ...review.pull_request, status: "merged" }, actions: [action("api"), action("ui")] };
  const repository = await mount(record, { mutate: vi.fn(async () => ({ review: merged, merge_intents: [intent()] })) });
  await submit();
  expect(document.querySelector("[data-pr-merge-result]")).toBeNull();
  panel.update(merged, { ...record, review: merged, merge_intents: [intent()] });
  expect(document.body.textContent).toContain("Merged locally; publication failed");
  expect(document.querySelector("[data-pr-merge-retry]").textContent).toContain("Retry publication");
  expect(repository.mutate.mock.calls.every(([verb]) => verb === "merge")).toBe(true);
});

it("retries retained publication with original sources and the newest version, even after a newer snapshot", async () => {
  const merged = { ...review, version: 12, pull_request: { ...review.pull_request, status: "merged", latest_published_snapshot_id: "snapshot-2" },
    actions: [action("api"), action("ui")] };
  const next = { ...record, review: merged, sync: record.sync.map((row) => ({ ...row, target_head: mergedHead })), merge_intents: [intent()] };
  const repository = await mount(next);
  expect(document.querySelector("[data-pr-merge-retry]").textContent).toBe("Retry publication");
  await document.querySelector("[data-pr-merge-retry]").onclick();
  expect(repository.mutate).toHaveBeenCalledExactlyOnceWith("merge", { ...params, expected_version: 12 });
});

it("labels a partial merge with failed publication as Retry saved merge and sends every original source", async () => {
  const ui = action("ui", "failed", "skipped");
  const partial = { ...record, review: { ...review, version: 7, actions: [action("api"), ui] }, merge_intents: [intent()] };
  const repository = await mount(partial);
  expect(document.querySelector("[data-pr-merge-retry]").textContent).toBe("Retry saved merge");
  await document.querySelector("[data-pr-merge-retry]").onclick();
  expect(repository.mutate).toHaveBeenCalledExactlyOnceWith("merge", { ...params, expected_version: 7 });
});

it("offers a separate current-target recovery plan after an opinion makes a partial merge retry stale", async () => {
  const partial = { ...record, review: { ...review, actions: [action("api"), action("ui", "failed", "skipped")] },
    merge_intents: [{ ...intent(), execution_version: 3 }] };
  const stale = new Error("PR version changed during merge; refresh the plan");
  const repository = await mount(partial, { mutate: vi.fn().mockRejectedValueOnce(stale).mockResolvedValue(undefined) });
  const newer = { ...partial, review: { ...partial.review, version: 7 },
    sync: partial.sync.map((row) => ({ ...row, target_head: row.directory_id === "api" ? mergedHead : oldHead })) };
  panel.update(newer.review, newer);
  await document.querySelector("[data-pr-merge-retry]").onclick();
  expect(document.querySelector("[data-pr-merge-error]").textContent).toContain(stale.message);
  const prepare = document.querySelector('[data-pr-merge-prepare="merge-1"]');
  expect(prepare?.textContent).toBe("Prepare a new merge plan");
  await prepare.onclick();
  expect(repository.mutate).toHaveBeenCalledTimes(1);
  expect(document.querySelector("[data-pr-merge-form]")).not.toBeNull();
  expect(field("push", "api").checked).toBe(true);
  expect(field("push", "ui").checked).toBe(true);
  expect(field("branch", "api").value).toBe("main");
  expect(field("push", "api").matches(":disabled")).toBe(true);
  expect(document.body.textContent).toContain("Current target:");
  panel.dispose();
  await mount(newer, repository);
  expect(document.querySelector("[data-pr-merge-form]")).not.toBeNull();
  expect(repository.mutate).toHaveBeenCalledTimes(1);
  await submit();
  expect(repository.mutate.mock.calls).toEqual([
    ["merge", { ...params, expected_version: 7 }],
    ["merge", { expected_version: 7, snapshot_id: snapshot.id,
      sources: [{ ...source("api"), expected_base_head: mergedHead }, source("ui")] }],
  ]);
  expect(document.querySelector('[data-pr-merge-result="merge-1"]').textContent).toContain("Publication refused");
});

it("highlights a partial merge and interruption without submitting after hydration or reconnect", async () => {
  await writeUiRecord(reviewActionDraftAddress(scope, "merge"), { selected: {}, submitted: params });
  const partial = { ...record, review: { ...review, actions: [action("api", "succeeded", "succeeded"), action("ui", "failed")] }, merge_intents: [intent("interrupted")] };
  const repository = await mount(partial);
  expect(document.body.textContent).toContain("Partially merged");
  expect(document.body.textContent).toContain("Interrupted");
  expect(document.querySelector("[data-pr-merge-form]")).toBeNull();
  panel.dispose(); await mount(partial, repository);
  panel.update(partial.review, partial);
  expect(repository.mutate).not.toHaveBeenCalled();
  await document.querySelector("[data-pr-merge-retry]").onclick();
  expect(repository.mutate).toHaveBeenCalledExactlyOnceWith("merge", params);
});

it("keeps an exact submitted draft through stale_version and never automatically retries", async () => {
  const stale = Object.assign(new Error("Review changed; inspect the current targets."), { code: "stale_version" });
  const repository = await mount(record, { mutate: vi.fn(async () => { throw stale; }) });
  change("push", "api", true);
  await submit();
  const saved = (await readUiRecord(reviewActionDraftAddress(scope, "merge"))).value;
  expect(saved.submitted).toEqual({ ...params, sources: [source("api"), source("ui", false)] });
  panel.dispose(); await mount({ ...record, review: { ...review, version: 9 } }, repository);
  expect(field("push", "api").checked).toBe(true);
  expect(repository.mutate).toHaveBeenCalledTimes(1);
});

it("does not offer publication retry once cached later Push successes settle the saved intent", async () => {
  const completed = { ...record, review: { ...review, pull_request: { ...review.pull_request, status: "merged" },
    actions: [action("api", "succeeded", "succeeded"), action("ui", "succeeded", "succeeded")] }, merge_intents: [intent()] };
  await mount(completed);
  expect(document.querySelector("[data-pr-merge-retry]")).toBeNull();
  expect(document.body.textContent).toContain("Publication complete");
});

it("allows an explicit fresh plan after stale admission without replaying the old preconditions", async () => {
  await writeUiRecord(reviewActionDraftAddress(scope, "merge"), { selected: {}, submitted: params });
  const next = { ...record, review: { ...review, version: 7 }, sync: record.sync.map((row) => ({ ...row, target_head: mergedHead })) };
  const repository = await mount(next);
  expect(repository.mutate).not.toHaveBeenCalled();
  expect(field("push", "api").checked).toBe(true);
  expect(document.body.textContent).toContain(`Saved expected target: ${oldHead}`);
  expect(document.body.textContent).toContain(`Current target: ${mergedHead}`);
  await document.querySelector("[data-pr-merge-new]").onclick();
  await submit();
  expect(repository.mutate).toHaveBeenCalledExactlyOnceWith("merge", { expected_version: 7, snapshot_id: snapshot.id,
    sources: ["api", "ui"].map((id) => ({ directory_id: id, expected_base_head: mergedHead })) });
});

it("does not offer a retry while saved Git work is still running", async () => {
  const running = { ...action("api"), status: "running" };
  const repository = await mount({ ...record, review: { ...review, actions: [running] }, merge_intents: [intent("interrupted")] });
  expect(document.querySelector("[data-pr-merge-retry]")).toBeNull();
  expect(document.querySelector("[data-pr-merge-form]")).toBeNull();
  expect(repository.mutate).not.toHaveBeenCalled();
});

it("requires durable integration and settled publication with no running or unresolved interrupted work before reclaim", () => {
  const merged = { ...review, pull_request: { ...review.pull_request, status: "merged" }, actions: [
    action("api", "succeeded", "succeeded"), action("ui", "succeeded", "succeeded"),
  ] };
  expect(settledMerge({ review: merged, merge_intents: [intent("succeeded")] })).toBe(true);
  expect(settledMerge({ review: merged, merge_intents: [intent("running")] })).toBe(false);
  expect(settledMerge({ review: merged, merge_intents: [intent("failed")] })).toBe(true);
  expect(settledMerge({ review: merged, merge_intents: [] })).toBe(false);
  expect(settledMerge({ review, merge_intents: [intent("succeeded")] })).toBe(false);
  expect(settledMerge({ review: { ...merged, actions: [action("api"), action("ui")] }, merge_intents: [intent("succeeded")] })).toBe(false);
});

it("resolves retained failed Push rows with the newer successful saved Push without forgetting the Merge", () => {
  const retained = intent("succeeded");
  retained.action_ids.push("retry-api", "retry-ui");
  const retry = (id) => ({ id: `retry-${id}`, directory_id: id, snapshot_id: snapshot.id, status: "succeeded", steps: [
    { kind: "push", remote: "origin", branch: "main", status: "succeeded", input_head: mergedHead, result_head: mergedHead,
      merge_action_id: `action-${id}` },
  ] });
  const merged = { ...review, pull_request: { ...review.pull_request, status: "merged" },
    actions: [action("api"), action("ui"), retry("api"), retry("ui")] };
  expect(settledMerge({ review: merged, merge_intents: [retained] })).toBe(true);
});

it("allows reclaim after a new successful merge despite a retained failure before integration", () => {
  const abandoned = { ...intent(), request_id: "old-failed", action_ids: ["failed-api", "failed-ui"] };
  const failed = (id) => ({ ...action(id, "failed"), id: `failed-${id}` });
  const merged = { ...review, pull_request: { ...review.pull_request, status: "merged" }, actions: [
    failed("api"), failed("ui"), action("api", "succeeded", "succeeded"), action("ui", "succeeded", "succeeded"),
  ] };
  expect(settledMerge({ review: merged, merge_intents: [abandoned, intent("succeeded")] })).toBe(true);
  expect(settledMerge({ review: merged, merge_intents: [{ ...abandoned, state: "running" }, intent("succeeded")] })).toBe(false);
  expect(settledMerge({ review: merged, merge_intents: [{ ...abandoned, state: "interrupted" }, intent("succeeded")] })).toBe(false);
  const uncertain = { ...failed("api"), status: "interrupted", steps: [{ kind: "merge", branch: "main", status: "interrupted" }] };
  expect(settledMerge({ review: { ...merged, actions: [uncertain, ...merged.actions.slice(1)] },
    merge_intents: [abandoned, intent("succeeded")] })).toBe(false);
});

it("rejects unrelated successful Push rows with wrong merge identity or merged result heads", () => {
  const retained = intent("succeeded");
  retained.action_ids.push("retry-api");
  const api = action("api");
  const ui = action("ui", "succeeded", "succeeded");
  const merged = { ...review, pull_request: { ...review.pull_request, status: "merged" } };
  const push = { kind: "push", remote: "origin", branch: "main", status: "succeeded", input_head: mergedHead,
    result_head: mergedHead, merge_action_id: "action-api" };
  for (const overrides of [
    { merge_action_id: "unrelated-merge" }, { merge_action_id: undefined },
    { input_head: oldHead }, { result_head: oldHead }, { result_head: undefined },
  ]) {
    const retry = { id: "retry-api", directory_id: "api", snapshot_id: snapshot.id, status: "succeeded", steps: [{ ...push, ...overrides }] };
    expect(settledMerge({ review: { ...merged, actions: [api, ui, retry] }, merge_intents: [retained] })).toBe(false);
  }
  const retry = { id: "retry-api", directory_id: "api", snapshot_id: snapshot.id, status: "succeeded", steps: [push] };
  expect(settledMerge({ review: { ...merged, actions: [api, ui, retry] }, merge_intents: [retained] })).toBe(true);
});
