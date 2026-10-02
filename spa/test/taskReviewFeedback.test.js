// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountTaskReviewFeedback, openTaskReviewer } from "../src/core/taskReviewFeedback.js";
import { timelineRows } from "../src/core/trackerTimeline.js";
import { timelineHtml } from "../src/core/trackerTaskRender.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const anchor = { snapshot_id: "snap-1", directory_id: "dir:1", path: "a<&.js", side: "new", line: 12 };
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const until = async (ready) => {
  for (let attempt = 0; attempt < 40 && !ready(); attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  expect(ready()).toBe(true);
};
const mount = (host, callRpc = vi.fn(async () => ({})), over = {}) => mountTaskReviewFeedback(host, {
  deviceId: "device", projectId: "project", taskId: "task", snapshot: { id: "snap-1", number: 2 },
  callRpc, ...over,
});

beforeEach(() => { document.body.innerHTML = ""; });

describe("task review feedback", () => {
  it("sends an anchored opinion and reply through the task comment verb", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    const callRpc = vi.fn(async () => ({}));
    const feedback = mount(host, callRpc);
    feedback.comment(anchor, "tc-1");
    host.querySelector("textarea").value = "Please adjust this";
    host.querySelector("textarea").dispatchEvent(new Event("input"));
    host.querySelector("select").value = "request_changes";
    host.querySelector("select").dispatchEvent(new Event("change"));
    host.querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
    await until(() => callRpc.mock.calls.length === 1);
    expect(callRpc).toHaveBeenCalledWith("tasks.comment", {
      task_id: "task", body: "Please adjust this", anchor, reply_to: "tc-1",
      opinion: { snapshot_id: "snap-1", verdict: "request_changes" },
    });
    await until(() => host.querySelector("textarea").value === "");
    feedback.dispose();
  });

  it("keeps a newer edit when a sent comment resolves, and restores it after remount", async () => {
    const host = document.createElement("div");
    document.body.append(host);
    let finish;
    const callRpc = vi.fn(() => new Promise((resolve) => { finish = resolve; }));
    const feedback = mount(host, callRpc);
    await flush();
    const field = host.querySelector("textarea");
    field.value = "sent";
    field.dispatchEvent(new Event("input"));
    host.querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
    await until(() => callRpc.mock.calls.length === 1);
    field.value = "new thought";
    field.dispatchEvent(new Event("input"));
    finish({});
    await flush();
    expect(field.value).toBe("new thought");
    feedback.dispose();
    const next = mount(host);
    await until(() => host.querySelector("textarea").value === "new thought");
    next.dispose();
  });

  it("renders old anchor metadata and a reply action after a review is replaced", () => {
    const rows = timelineRows([{ type: "comment", id: "tc-2", body: "old", author: { kind: "user" },
      anchor, opinion: { snapshot_id: "snap-1", verdict: "approve" }, reply_to: "tc-1" }]);
    const html = timelineHtml(rows, {});
    expect(html).toContain("Snapshot snap-1");
    expect(html).toContain("Approved");
    expect(html).toContain("data-review-anchor=");
    expect(html).toContain("data-review-reply=\"tc-2\"");
    expect(html).toContain("a&lt;&amp;.js");
  });

  it("opens the ordinary assignee picker with the snapshot in its note", async () => {
    const picker = openTaskReviewer({
      task: { id: "task", number: 7, title: "Check API", assignee: null },
      snapshot: { id: "snap-1", number: 2 },
      feed: { items: [], workspaces: [] }, projectKey: "device|project",
      callRpc: vi.fn(),
    });
    expect(document.querySelector("#task-assign-note").value).toContain("snapshot 2 of task #7");
    await picker.close();
  });

  it("keeps the caret when typing in the middle of a draft", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const feedback = mount(host);
    const field = host.querySelector("textarea");
    field.value = "ab";
    field.setSelectionRange(1, 1);
    field.dispatchEvent(new Event("input"));
    expect(field.selectionStart).toBe(1);
    feedback.dispose();
  });

  it("does not clear another mount's draft when the first comment succeeds", async () => {
    const firstHost = document.createElement("div");
    const secondHost = document.createElement("div");
    document.body.append(firstHost, secondHost);
    let finish;
    const callRpc = vi.fn(() => new Promise((resolve) => { finish = resolve; }));
    const options = { taskId: "two-mounts" };
    const first = mount(firstHost, callRpc, options);
    const second = mount(secondHost, vi.fn(), options);
    await flush();
    const firstField = firstHost.querySelector("textarea");
    firstField.value = "sent";
    firstField.dispatchEvent(new Event("input"));
    firstHost.querySelector("form").dispatchEvent(new Event("submit", { cancelable: true }));
    await until(() => callRpc.mock.calls.length === 1);
    const secondField = secondHost.querySelector("textarea");
    secondField.value = "another tab's draft";
    secondField.dispatchEvent(new Event("input"));
    await new Promise((resolve) => setTimeout(resolve, 150));
    finish({});
    await flush();
    first.dispose();
    second.dispose();
    const remount = mount(secondHost, vi.fn(), options);
    await until(() => secondHost.querySelector("textarea").value === "another tab's draft");
    remount.dispose();
  });
});
