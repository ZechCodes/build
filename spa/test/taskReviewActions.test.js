// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { wipeUiRecords, readUiRecord, writeUiRecord } from "../src/core/localUiStore.js";
import { uiAddress } from "../src/core/localUiState.js";
import { mountTaskReviewActions } from "../src/core/taskReviewActions.js";
import fixture from "../../fixtures/api/v1/tasks.review.get.json";

const snapshot = fixture.result.review.snapshots[0];
const destination = (directory_id) => ({ snapshot_id: snapshot.id, directory_id, source_path: `/sources/${directory_id}`,
  branches: ["main", "dev"], remotes: [{ name: "origin", branches: ["main", "release"] }], live_head: "3".repeat(40) });
const base = { ...fixture.result.review, destinations: [destination("dir-api"), destination("dir-missing")], actions: [] };
let sheet;
const mount = (review, repository, support = { act: true, complete: true }) => {
  document.body.innerHTML = '<div id="actions"></div>';
  sheet = mountTaskReviewActions(document.querySelector("#actions"), { deviceId: "actions-device", projectId: "proj-1", taskId: "task-1",
    snapshot, review, support, repository });
};
const choose = (id, kind) => {
  const input = document.querySelector(`[data-review-${kind}="${id}"]`);
  input.checked = true; input.dispatchEvent(new Event("change"));
};
const submit = () => document.querySelector('[data-review-act]').dispatchEvent(new Event("submit", { cancelable: true }));
const row = (id, directory_id, steps, status = "succeeded") => ({ id, snapshot_id: snapshot.id, directory_id,
  source_name: directory_id, source_path: `/sources/${directory_id}`, actor: { kind: "user" }, started_at: "2026-10-02T12:00:00Z",
  status, steps });
const step = (kind, status = "succeeded") => ({ kind, branch: kind === "merge" ? "dev" : "main", remote: kind === "push" ? "origin" : undefined, status });
beforeEach(async () => { sheet?.dispose(); await wipeUiRecords(); });
afterEach(() => sheet?.dispose());

it("gates the form on act support while showing cached results", () => {
  mount({ ...base, actions: [row("a", "dir-api", [step("merge")])] }, { mutate: vi.fn() }, { act: false, complete: true });
  expect(document.querySelector('[data-review-act]')).toBeNull();
  expect(document.body.textContent).toContain("succeeded");
});

it("sends independent Merge and Push selections and completes from recorded successes", async () => {
  const repository = { mutate: vi.fn(async () => {}) };
  mount(base, repository);
  choose("dir-api", "merge"); choose("dir-api", "push"); choose("dir-missing", "push");
  submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", { expected_version: 1, snapshot_id: snapshot.id,
    sources: [{ directory_id: "dir-api", merge: { branch: "main" }, push: { remote: "origin", branch: "build/review" } },
      { directory_id: "dir-missing", push: { remote: "origin", branch: "main" } }] }));
  sheet.update({ ...base, version: 5, actions: [row("a", "dir-api", [{ ...step("merge"), branch: "main" }, { ...step("push"), branch: "build/review" }]), row("b", "dir-missing", [step("push")])] });
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("complete", { expected_version: 5,
    description: expect.stringContaining("merged") }));
  expect(repository.mutate.mock.calls.filter(([verb]) => verb === "act")).toHaveLength(1);
});

it("shows failed Push and explicitly retries only Push from the successful Merge tip", async () => {
  const failed = row("action-1", "dir-api", [{ ...step("merge"), branch: "dev" }, { ...step("push", "failed"), branch: "release" }], "failed");
  const repository = { mutate: vi.fn(async () => {}) };
  mount({ ...base, version: 4, actions: [failed] }, repository);
  expect(document.body.textContent).toContain("failed");
  document.querySelector('[data-review-retry-push="action-1"]').click();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", { expected_version: 4, snapshot_id: snapshot.id,
    sources: [{ directory_id: "dir-api", push: { remote: "origin", branch: "release", merge_action_id: "action-1" } }] }));
  expect(repository.mutate.mock.calls.filter(([verb]) => verb === "complete")).toHaveLength(0);
  sheet.update({ ...base, version: 5, actions: [failed, row("action-2", "dir-api", [{ ...step("push"), branch: "release" }])] });
  await vi.waitFor(() => expect(repository.mutate.mock.calls.filter(([verb]) => verb === "complete")).toHaveLength(1));
});

it("does not retry interrupted work on mount or reconnect", async () => {
  const repository = { mutate: vi.fn() };
  const interrupted = { ...base, actions: [row("a", "dir-api", [step("merge", "interrupted")], "interrupted")] };
  mount(interrupted, repository);
  sheet.dispose(); mount(interrupted, repository);
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(repository.mutate).not.toHaveBeenCalled();
});

it("keeps draft choices after a stale refusal and reconnect", async () => {
  const stale = Object.assign(new Error("Review changed"), { code: "stale_version" });
  const repository = { mutate: vi.fn(async () => { throw stale; }) };
  mount(base, repository); choose("dir-api", "merge"); submit();
  await vi.waitFor(() => expect(document.querySelector('[data-review-act-error]').textContent).toContain("Review changed"));
  sheet.dispose(); mount({ ...base, version: 2 }, { mutate: vi.fn() });
  await vi.waitFor(() => expect(document.querySelector('[data-review-merge="dir-api"]').checked).toBe(true));
});

it("finishes saved successful work after reconnect without replaying Git", async () => {
  const repository = { mutate: vi.fn(async () => {}) };
  mount(base, repository); choose("dir-api", "merge"); submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", expect.anything()));
  sheet.dispose();
  const success = row("action-3", "dir-api", [{ ...step("merge"), branch: "main" }]);
  mount({ ...base, version: 3, actions: [success] }, repository);
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("complete", { expected_version: 3,
    description: expect.stringContaining("merged") }));
  expect(repository.mutate.mock.calls.filter(([verb]) => verb === "act")).toHaveLength(1);
});

it("does not complete a multi-source review while another source remains failed", async () => {
  const repository = { mutate: vi.fn(async () => {}) };
  mount(base, repository); choose("dir-api", "merge"); choose("dir-missing", "push"); submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", expect.anything()));
  const failed = row("failed", "dir-missing", [step("push", "failed")], "failed");
  sheet.update({ ...base, version: 4, actions: [row("success", "dir-api", [{ ...step("merge"), branch: "main" }]), failed] });
  await new Promise((resolve) => setTimeout(resolve, 25));
  expect(repository.mutate.mock.calls.filter(([verb]) => verb === "complete")).toHaveLength(0);
});

it("completes the original multi-source choice after its failed Push is explicitly retried", async () => {
  const repository = { mutate: vi.fn(async () => {}) };
  mount(base, repository); choose("dir-api", "merge"); choose("dir-missing", "push"); submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", expect.anything()));
  const failed = row("failed", "dir-missing", [step("push", "failed")], "failed");
  const api = row("api", "dir-api", [{ ...step("merge"), branch: "main" }]);
  sheet.update({ ...base, version: 4, actions: [api, failed] });
  await new Promise((resolve) => setTimeout(resolve, 25));
  sheet.dispose();
  mount({ ...base, version: 4, actions: [api, failed] }, repository);
  await vi.waitFor(() => expect(document.querySelector('[data-review-push="dir-missing"]').checked).toBe(true));
  const address = uiAddress({ deviceId: "actions-device", entityId: "proj-1", view: "task-review-actions", kind: "git-draft",
    sub: JSON.stringify(["task-1", snapshot.id]) });
  expect((await readUiRecord(address)).value.intent.sources).toHaveLength(2);
  expect((await readUiRecord(address)).value.intent.before).toEqual([]);
  document.querySelector('[data-review-retry-push="failed"]').click();
  await vi.waitFor(() => expect(repository.mutate.mock.calls.filter(([verb]) => verb === "act")).toHaveLength(2));
  sheet.update({ ...base, version: 6, actions: [api, failed, row("retry", "dir-missing", [step("push")])] });
  await vi.waitFor(() => expect(repository.mutate.mock.calls.filter(([verb]) => verb === "complete")).toHaveLength(1));
});

it("keeps the original Merge tip through repeated failed Push retries", async () => {
  const merged = row("merge-row", "dir-api", [{ ...step("merge"), result_head: "4".repeat(40) },
    { ...step("push", "failed"), merge_action_id: "merge-row" }], "failed");
  const retry = row("retry-row", "dir-api", [{ ...step("push", "failed"), merge_action_id: "merge-row",
    input_head: "4".repeat(40) }], "failed");
  const repository = { mutate: vi.fn(async () => {}) };
  mount({ ...base, version: 5, actions: [merged, retry] }, repository);
  document.querySelector('[data-review-retry-push="retry-row"]').click();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", { expected_version: 5, snapshot_id: snapshot.id,
    sources: [{ directory_id: "dir-api", push: { remote: "origin", branch: "main", merge_action_id: "merge-row" } }] }));
});

it("does not count a Push of the wrong head as completing Merge and Push", async () => {
  const repository = { mutate: vi.fn(async () => {}) };
  mount(base, repository); choose("dir-api", "merge"); choose("dir-api", "push"); submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", expect.anything()));
  const failed = row("merge-row", "dir-api", [{ ...step("merge"), branch: "main", result_head: "4".repeat(40) },
    { ...step("push", "failed"), branch: "build/review", merge_action_id: "merge-row" }], "failed");
  sheet.update({ ...base, version: 3, actions: [failed] });
  await new Promise((resolve) => setTimeout(resolve, 25));
  document.querySelector('[data-review-retry-push="merge-row"]').click();
  await vi.waitFor(() => expect(repository.mutate.mock.calls.filter(([verb]) => verb === "act")).toHaveLength(2));
  const wrong = row("wrong", "dir-api", [{ ...step("push"), branch: "build/review", input_head: "2".repeat(40), merge_action_id: "merge-row" }]);
  sheet.update({ ...base, version: 5, actions: [failed, wrong] });
  await new Promise((resolve) => setTimeout(resolve, 30));
  expect(repository.mutate.mock.calls.filter(([verb]) => verb === "complete")).toHaveLength(0);
});

it("keeps a disconnected Retry Push pending while its recorded row runs", async () => {
  const repository = { mutate: vi.fn(async (verb) => {
    if (verb === "act" && repository.mutate.mock.calls.filter(([name]) => name === "act").length === 2) throw new Error("offline");
  }) };
  mount(base, repository); choose("dir-api", "merge"); choose("dir-api", "push"); submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", expect.anything()));
  const failed = row("merge-row", "dir-api", [{ ...step("merge"), branch: "main", result_head: "4".repeat(40) },
    { ...step("push", "failed"), branch: "build/review", merge_action_id: "merge-row" }], "failed");
  sheet.update({ ...base, version: 3, actions: [failed] });
  await new Promise((resolve) => setTimeout(resolve, 30));
  document.querySelector('[data-review-retry-push="merge-row"]').click();
  await vi.waitFor(() => expect(document.querySelector('[data-review-act-error]').textContent).toContain("offline"));
  const retry = row("retry-row", "dir-api", [{ ...step("push", "running"), branch: "build/review", input_head: "4".repeat(40),
    merge_action_id: "merge-row" }], "running");
  sheet.update({ ...base, version: 4, actions: [failed, retry] });
  await new Promise((resolve) => setTimeout(resolve, 220));
  const address = uiAddress({ deviceId: "actions-device", entityId: "proj-1", view: "task-review-actions", kind: "git-draft",
    sub: JSON.stringify(["task-1", snapshot.id]) });
  expect((await readUiRecord(address)).value.intent.paused).toBe(false);
  sheet.dispose();
  mount({ ...base, version: 4, actions: [failed, retry] }, repository);
  await vi.waitFor(() => expect(document.querySelector('[data-review-act]')).not.toBeNull());
  sheet.update({ ...base, version: 5, actions: [failed, { ...retry, status: "succeeded",
    steps: [{ ...retry.steps[0], status: "succeeded" }] }] });
  await vi.waitFor(() => expect(repository.mutate.mock.calls.filter(([verb]) => verb === "complete")).toHaveLength(1));
  expect(repository.mutate.mock.calls.filter(([verb]) => verb === "act")).toHaveLength(2);
});

it("completes selected success despite an unrelated historical failed source", async () => {
  const old = row("old", "dir-missing", [step("push", "failed")], "failed");
  const repository = { mutate: vi.fn(async () => {}) };
  mount({ ...base, version: 2, actions: [old] }, repository);
  choose("dir-api", "merge"); submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", expect.anything()));
  sheet.update({ ...base, version: 4, actions: [old, row("new", "dir-api", [{ ...step("merge"), branch: "main" }])] });
  await vi.waitFor(() => expect(repository.mutate.mock.calls.some(([verb]) => verb === "complete")).toBe(true));
});

it("keeps Git controls available on a completed review", () => {
  mount({ ...base, state: "completed" }, { mutate: vi.fn() });
  expect(document.querySelector('[data-review-act]')).not.toBeNull();
});

it("preserves typing focus and cursor through saved draft and review updates", async () => {
  const repository = { mutate: vi.fn() };
  mount(base, repository);
  document.querySelector('[data-review-act-sheet]').open = true;
  const branch = document.querySelector('[data-review-push-branch="dir-api"]');
  branch.focus(); branch.value = "feature/release"; branch.setSelectionRange(8, 8);
  branch.dispatchEvent(new Event("input"));
  await new Promise((resolve) => setTimeout(resolve, 240));
  sheet.update({ ...base, version: 2 });
  const restored = document.querySelector('[data-review-push-branch="dir-api"]');
  expect(document.activeElement).toBe(restored);
  expect(restored.value).toBe("feature/release");
  expect(restored.selectionStart).toBe(8);
});

it("retries completion on a later same-version refresh after a disconnected attempt", async () => {
  const repository = { mutate: vi.fn(async (verb) => { if (verb === "complete" && repository.mutate.mock.calls.filter(([name]) => name === "complete").length === 1) throw new Error("offline"); }) };
  mount(base, repository); choose("dir-api", "merge"); submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", expect.anything()));
  const success = { ...base, version: 3, actions: [row("done", "dir-api", [{ ...step("merge"), branch: "main" }])] };
  sheet.update(success);
  await vi.waitFor(() => expect(document.querySelector('[data-review-act-error]').textContent).toContain("offline"));
  sheet.update(success);
  await vi.waitFor(() => expect(repository.mutate.mock.calls.filter(([verb]) => verb === "complete")).toHaveLength(2));
  const address = uiAddress({ deviceId: "actions-device", entityId: "proj-1", view: "task-review-actions", kind: "git-draft",
    sub: JSON.stringify(["task-1", snapshot.id]) });
  await vi.waitFor(async () => expect((await readUiRecord(address))?.value?.intent).toBeNull());
});

it("does not write a late completion after the sheet is disposed", async () => {
  let resolveCompletion;
  const completion = new Promise((resolve) => { resolveCompletion = resolve; });
  const onTaskChanged = vi.fn();
  const repository = { mutate: vi.fn(async (verb) => verb === "complete" ? completion : undefined) };
  document.body.innerHTML = '<div id="actions"></div>';
  sheet = mountTaskReviewActions(document.querySelector("#actions"), { deviceId: "actions-device", projectId: "proj-1",
    taskId: "task-1", snapshot, review: base, support: { act: true, complete: true }, repository, onTaskChanged });
  choose("dir-api", "merge"); submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", expect.anything()));
  sheet.update({ ...base, version: 3, actions: [row("done", "dir-api", [{ ...step("merge"), branch: "main" }])] });
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("complete", expect.anything()));
  const address = uiAddress({ deviceId: "actions-device", entityId: "proj-1", view: "task-review-actions", kind: "git-draft",
    sub: JSON.stringify(["task-1", snapshot.id]) });
  sheet.dispose();
  resolveCompletion();
  await new Promise((resolve) => setTimeout(resolve, 230));
  expect((await readUiRecord(address)).value.intent).not.toBeNull();
  expect(onTaskChanged).not.toHaveBeenCalled();
});

it("settles a late completion refusal without touching a disposed sheet", async () => {
  let rejectCompletion;
  const completion = new Promise((_, reject) => { rejectCompletion = reject; });
  const repository = { mutate: vi.fn(async (verb) => verb === "complete" ? completion : undefined) };
  mount(base, repository); choose("dir-api", "merge"); submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", expect.anything()));
  sheet.update({ ...base, version: 3, actions: [row("done", "dir-api", [{ ...step("merge"), branch: "main" }])] });
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("complete", expect.anything()));
  const heldHtml = document.querySelector("#actions").innerHTML;
  sheet.dispose();
  rejectCompletion(new Error("late refusal"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(document.querySelector("#actions").innerHTML).toBe(heldHtml);
});

it("keeps an automatic completion summary within 2,000 UTF-8 bytes", async () => {
  const long = "é".repeat(1100);
  const destination = { ...base.destinations[0], branches: [long] };
  const repository = { mutate: vi.fn(async () => {}) };
  mount({ ...base, destinations: [destination] }, repository);
  choose("dir-api", "merge"); submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", expect.anything()));
  sheet.update({ ...base, version: 3, actions: [row("done", "dir-api", [{ ...step("merge"), branch: long }])] });
  await vi.waitFor(() => expect(repository.mutate.mock.calls.some(([verb]) => verb === "complete")).toBe(true));
  const description = repository.mutate.mock.calls.find(([verb]) => verb === "complete")[1].description;
  expect(new TextEncoder().encode(description).length).toBeLessThanOrEqual(2000);
});

it("submits the visible Merge branch and remote when saved options disappeared", async () => {
  const address = uiAddress({ deviceId: "actions-device", entityId: "proj-1", view: "task-review-actions", kind: "git-draft",
    sub: JSON.stringify(["task-1", snapshot.id]) });
  await writeUiRecord(address, { selected: { "dir-api": { merge: true, push: true, mergeBranch: "removed",
    remote: "removed", pushBranch: "feature/release" } }, intent: null });
  const repository = { mutate: vi.fn(async () => {}) };
  mount(base, repository);
  await vi.waitFor(() => expect(document.querySelector('[data-review-merge="dir-api"]').checked).toBe(true));
  expect(document.querySelector('[data-review-merge-branch="dir-api"]').value).toBe("main");
  expect(document.querySelector('[data-review-remote="dir-api"]').value).toBe("origin");
  expect(document.querySelector('[data-review-push-branch="dir-api"]').value).toBe("feature/release");
  submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", { expected_version: 1, snapshot_id: snapshot.id,
    sources: [{ directory_id: "dir-api", merge: { branch: "main" }, push: { remote: "origin", branch: "feature/release" } }] }));
});

it("hides a failed Push retry after a newer success for the same Merge and destination", () => {
  const failed = row("merge-row", "dir-api", [{ ...step("merge"), result_head: "4".repeat(40) },
    { ...step("push", "failed"), merge_action_id: "merge-row" }], "failed");
  const success = row("push-row", "dir-api", [{ ...step("push"), input_head: "4".repeat(40), merge_action_id: "merge-row" }]);
  mount({ ...base, actions: [failed, success] }, { mutate: vi.fn() });
  expect(document.querySelector('[data-review-retry-push="merge-row"]')).toBeNull();
  expect(document.querySelector('[data-review-result-status="succeeded"]')).not.toBeNull();
});

it("labels interrupted work with guidance and distinct result states", () => {
  mount({ ...base, actions: [row("failed", "dir-api", [step("push", "failed")], "failed"),
    row("interrupted", "dir-api", [step("merge", "interrupted")], "interrupted"),
    row("succeeded", "dir-api", [step("merge")])] }, { mutate: vi.fn() });
  expect(document.body.textContent).toContain("Interrupted: check and retry, or mark complete");
  expect([...document.querySelectorAll('[data-review-result-status]')].map((node) => node.dataset.reviewResultStatus))
    .toEqual(["failed", "interrupted", "succeeded"]);
});

it("defaults Push branch to the snapshot source branch", async () => {
  const repository = { mutate: vi.fn(async () => {}) };
  mount(base, repository);
  expect(document.querySelector('[data-review-push-branch="dir-api"]').value).toBe("build/review");
  choose("dir-api", "push"); submit();
  await vi.waitFor(() => expect(repository.mutate).toHaveBeenCalledWith("act", { expected_version: 1, snapshot_id: snapshot.id,
    sources: [{ directory_id: "dir-api", push: { remote: "origin", branch: "build/review" } }] }));
});
