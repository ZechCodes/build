// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let createTrackerIssueDetailsFeed, issueRecord, readIssueRecord, writeIssueRecord;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  ({ createTrackerIssueDetailsFeed } = await import("../src/core/trackerIssueDetailsFeed.js"));
  ({ issueRecord, readIssueRecord, writeIssueRecord } = await import("../src/core/trackerCache.js"));
});

const issue = (id, updated_at) => ({ id, updated_at });
const waitFor = (condition) => vi.waitFor(() => expect(condition()).toBe(true));

describe("tracker issue details feed", () => {
  it("takes a current detail from the cache without a wire read", async () => {
    const detail = issueRecord(issue("one", "2026-09-01T12:00:00Z"), [{ id: "ic-1" }]);
    await writeIssueRecord("dev", "project", "one", detail);
    const callRpc = vi.fn();
    const onChange = vi.fn();
    const feed = createTrackerIssueDetailsFeed({ deviceId: "dev", projectId: "project", callRpc, onChange });

    await feed.updateIssues([issue("one", "2026-09-01T12:00:00Z")]);

    expect(feed.read().get("one")).toEqual(detail);
    expect(callRpc).not.toHaveBeenCalled();
    expect(onChange).toHaveBeenCalled();
    feed.dispose();
  });

  it("keeps an older cached detail visible until a refreshed record announces", async () => {
    const old = issueRecord(issue("one", "2026-09-01T12:00:00Z"), [{ id: "ic-old" }]);
    await writeIssueRecord("dev", "project", "one", old);
    let answer;
    const callRpc = vi.fn(() => new Promise((resolve) => { answer = resolve; }));
    const feed = createTrackerIssueDetailsFeed({ deviceId: "dev", projectId: "project", callRpc });

    await feed.updateIssues([issue("one", "2026-09-02T12:00:00Z")]);
    expect(feed.read().get("one")).toEqual(old);
    expect(callRpc).toHaveBeenCalledWith("issues.get", { issue_id: "one" });

    const fresh = issueRecord(issue("one", "2026-09-02T12:00:00Z"), [{ id: "ic-new" }]);
    answer(fresh);
    await waitFor(() => feed.read().get("one")?.timeline?.[0]?.id === "ic-new");
    expect(await readIssueRecord("dev", "project", "one")).toEqual(fresh);
    feed.dispose();
  });

  it("bounds live reads and drops results after disposal", async () => {
    const answers = [];
    const callRpc = vi.fn(() => new Promise((resolve) => { answers.push(resolve); }));
    const feed = createTrackerIssueDetailsFeed({ deviceId: "dev", projectId: "project", callRpc });

    await feed.updateIssues(Array.from({ length: 7 }, (_, index) => issue(`issue-${index}`, "2026-09-01T12:00:00Z")));
    expect(callRpc).toHaveBeenCalledTimes(4);
    answers[0]({ issue: issue("issue-0", "2026-09-01T12:00:00Z"), timeline: [] });
    await waitFor(() => callRpc.mock.calls.length === 5);

    feed.dispose();
    answers[1]({ issue: issue("issue-1", "2026-09-01T12:00:00Z"), timeline: [] });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(await readIssueRecord("dev", "project", "issue-1")).toBeNull();
    expect(callRpc).toHaveBeenCalledTimes(5);
  });

  it("ignores a malformed answer and does not repeat a failed version", async () => {
    const callRpc = vi.fn(async () => ({}));
    const feed = createTrackerIssueDetailsFeed({ deviceId: "dev", projectId: "project", callRpc });
    const listed = [issue("one", "2026-09-01T12:00:00Z")];

    await feed.updateIssues(listed);
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(1));
    await feed.updateIssues(listed);

    expect(await readIssueRecord("dev", "project", "one")).toBeNull();
    expect(callRpc).toHaveBeenCalledTimes(1);
    feed.dispose();
  });
});
