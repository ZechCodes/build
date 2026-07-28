// @vitest-environment jsdom
import { describe, it, expect, vi } from "vitest";
import { bucketIssues, issuesHtml, mountIssuesTab } from "../src/views/issues.js";

const NOW = Date.parse("2026-07-25T12:00:00Z");
const plan = (over = {}) => ({
  plan_id: "pl1",
  project_id: "p1",
  goal: "500 at /admin/usage",
  state: "plan_review",
  updated_at: "2026-07-25T11:00:00Z",
  stages: [],
  ...over,
});

describe("bucketIssues", () => {
  it("keeps only this project's issues", () => {
    const { open } = bucketIssues([plan(), plan({ plan_id: "pl2", project_id: "p2" })], "p1", NOW);
    expect(open.map((p) => p.plan_id)).toEqual(["pl1"]);
  });

  it("closes abandoned issues and leaves every live state open", () => {
    const plans = [
      plan({ plan_id: "a", state: "abandoned" }),
      plan({ plan_id: "b", state: "approved" }),
      plan({ plan_id: "c", state: "drafting" }),
    ];
    const { open, closed } = bucketIssues(plans, "p1", NOW);
    expect(closed.map((p) => p.plan_id)).toEqual(["a"]);
    expect(open.map((p) => p.plan_id).sort()).toEqual(["b", "c"]);
  });

  it("orders each bucket newest first", () => {
    const plans = [
      plan({ plan_id: "old", updated_at: "2026-07-20T12:00:00Z" }),
      plan({ plan_id: "new", updated_at: "2026-07-25T11:59:00Z" }),
    ];
    expect(bucketIssues(plans, "p1", NOW).open.map((p) => p.plan_id)).toEqual(["new", "old"]);
  });
});

describe("issuesHtml", () => {
  it("renders a row per issue with its state chip", () => {
    const html = issuesHtml(bucketIssues([plan()], "p1", NOW));
    expect(html).toContain('data-plan="pl1"');
    expect(html).toContain("500 at /admin/usage");
    expect(html).toContain("OPEN");
  });

  it("says an issue is being implemented when a run holds it", () => {
    const html = issuesHtml(bucketIssues([plan({ state: "approved", active_run_id: "run-1" })], "p1", NOW));
    expect(html).toContain("implementing");
  });

  // A UI that gives directions to its own buttons has already lost: the empty
  // state carries the verb, it does not point at where the verb lives.
  it("offers the verb itself in the empty state", () => {
    const html = issuesHtml({ open: [], closed: [] });
    expect(html).toContain("No issues yet");
    expect(html).toContain("data-newissue");
    expect(html).not.toMatch(/corner|button in the/i);
  });

  it("escapes issue text", () => {
    const html = issuesHtml(bucketIssues([plan({ goal: '<img src=x onerror="alert(1)">' })], "p1", NOW));
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
  });
});

describe("mountIssuesTab", () => {
  it("wires the empty state's button to the issue sheet", async () => {
    const host = document.createElement("div");
    const opened = [];
    mountIssuesTab(host, {
      projectId: "p1",
      callRpc: async () => ({ plans: [] }),
      navigate: () => {},
      onNewIssue: () => opened.push("sheet"),
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    host.querySelector("[data-newissue]").click();
    expect(opened).toEqual(["sheet"]);
  });

  it("routes a click to that issue's surface and stops polling on dispose", async () => {
    vi.useFakeTimers();
    const host = document.createElement("div");
    const callRpc = vi.fn(async () => ({ plans: [plan()] }));
    const routes = [];
    const ctl = mountIssuesTab(host, { projectId: "p1", callRpc, navigate: (r) => routes.push(r) });
    await vi.advanceTimersByTimeAsync(0);
    host.querySelector(".issue-row").click();
    expect(routes).toEqual([{ name: "plan", projectId: "p1", id: "pl1", tab: "review" }]);
    const callsBefore = callRpc.mock.calls.length;
    ctl.dispose();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(callRpc.mock.calls.length).toBe(callsBefore);
    vi.useRealTimers();
  });
});
