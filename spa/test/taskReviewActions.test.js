// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { beforeEach, expect, it, vi } from "vitest";
import { wipeUiRecords } from "../src/core/localUiStore.js";
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
    sources: [{ directory_id: "dir-api", merge: { branch: "main" }, push: { remote: "origin", branch: "main" } },
      { directory_id: "dir-missing", push: { remote: "origin", branch: "main" } }] }));
  sheet.update({ ...base, version: 5, actions: [row("a", "dir-api", [{ ...step("merge"), branch: "main" }, step("push")]), row("b", "dir-missing", [step("push")])] });
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
