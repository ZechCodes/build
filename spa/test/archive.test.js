// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
import { archiveHtml, mountArchiveTab, normalizeArchive } from "../src/views/archive.js";

const plan = (overrides = {}) => ({
  plan_id: "plan-1",
  goal: "Ship the archive",
  state: "approved",
  archived_at: "2026-07-25T11:00:00Z",
  stages: [{ id: "one" }, { id: "two" }],
  ...overrides,
});

const worktree = (overrides = {}) => ({
  worktree_id: "worktree-1",
  branch: "feature/archive",
  path: "/repo/archive",
  head_sha: "abc1234",
  upstream: "origin/feature/archive",
  unpushed: 2,
  dirty_files: 3,
  action: "merge",
  archived_at: "2026-07-25T12:00:00Z",
  ...overrides,
});

afterEach(() => {
  vi.useRealTimers();
});

describe("normalizeArchive", () => {
  it("groups plans and worktrees and sorts each newest archive first", () => {
    const archive = normalizeArchive({
      plans: [
        plan({ plan_id: "old-plan", archived_at: "2026-07-20T12:00:00Z" }),
        plan({ plan_id: "new-plan", archived_at: "2026-07-26T12:00:00Z" }),
      ],
      worktrees: [
        worktree({ worktree_id: "old-worktree", archived_at: "2026-07-19T12:00:00Z" }),
        worktree({ worktree_id: "new-worktree", archived_at: "2026-07-27T12:00:00Z" }),
      ],
    });

    expect(archive.plans.map((item) => item.id)).toEqual(["new-plan", "old-plan"]);
    expect(archive.worktrees.map((item) => item.id)).toEqual(["new-worktree", "old-worktree"]);
  });

  it("puts missing and invalid archive dates last without mutating the response", () => {
    const plans = [
      plan({ plan_id: "missing", archived_at: undefined }),
      plan({ plan_id: "valid", archived_at: "2026-07-25T11:00:00Z" }),
      plan({ plan_id: "invalid", archived_at: "not-a-date" }),
    ];
    const before = [...plans];

    expect(normalizeArchive({ plans }).plans.map((item) => item.id)).toEqual(["valid", "missing", "invalid"]);
    expect(plans).toEqual(before);
  });

  it("normalizes retained stage metadata and the backend worktree fields", () => {
    const archive = normalizeArchive({
      plans: [
        plan({ plan_id: undefined, title: "Legacy plan", goal: undefined, stages: undefined, stage_count: 4 }),
      ],
      worktrees: [
        worktree({
          worktree_id: "legacy-worktree",
          branch: null,
          head_sha: "deadbee",
        }),
      ],
    });

    expect(archive.plans[0]).toMatchObject({ id: null, goal: "Legacy plan", stageCount: 4 });
    expect(archive.worktrees[0]).toMatchObject({
      id: "legacy-worktree",
      branch: null,
      headSha: "deadbee",
    });
  });

  it("treats absent, null, and malformed buckets as empty", () => {
    expect(normalizeArchive()).toEqual({ plans: [], worktrees: [] });
    expect(normalizeArchive({ plans: null, worktrees: {} })).toEqual({ plans: [], worktrees: [] });
  });
});

describe("archiveHtml", () => {
  it("always presents explicit Plans and Worktrees buckets with calm empty states", () => {
    const html = archiveHtml({ plans: [], worktrees: [] });

    expect(html).toContain("PLANS");
    expect(html).toContain("WORKTREES");
    expect(html).toContain("No archived plans yet.");
    expect(html).toContain("No archived worktrees yet.");
  });

  it("shows plan history and only makes retained plans with an id navigable", () => {
    const html = archiveHtml({
      plans: [plan(), plan({ plan_id: undefined, goal: "Legacy record", stages: undefined, retained_stages: [{}] })],
    });

    expect(html).toContain("Ship the archive");
    expect(html).toContain("Former state: approved");
    expect(html).toContain("2 stages");
    expect(html).toContain('data-plan="plan-1"');
    expect(html).not.toContain('data-plan=""');
    expect(html).toContain("Legacy record");
    expect(html).toContain("1 stage");
  });

  it("shows detached worktree history and available git metadata without live actions", () => {
    const html = archiveHtml({ worktrees: [worktree({ branch: null })] });

    expect(html).toContain("(detached)");
    expect(html).toContain("Finish action: merge");
    expect(html).toContain("/repo/archive");
    expect(html).toContain("abc1234");
    expect(html).toContain("origin/feature/archive");
    expect(html).toContain("2 unpushed");
    expect(html).toContain("3 dirty");
    expect(html).not.toContain("<button");
    expect(html).not.toContain("data-wt=");
  });

  it("escapes every server-provided string in plan and worktree records", () => {
    const attack = '<img src=x onerror="alert(1)">';
    const html = archiveHtml({
      plans: [plan({ plan_id: attack, goal: attack, state: attack, archived_at: attack })],
      worktrees: [worktree({
        worktree_id: attack,
        branch: attack,
        path: attack,
        head_sha: attack,
        upstream: attack,
        unpushed: attack,
        dirty_files: attack,
        action: attack,
        archived_at: attack,
      })],
    });

    expect(html).not.toContain("<img");
    expect(html).not.toContain('onerror="alert(1)"');
    expect(html).toContain("&lt;img src=x onerror=&quot;alert(1)&quot;&gt;");
  });
});

describe("mountArchiveTab", () => {
  it("fetches this project and routes retained plan clicks to plan review", async () => {
    const host = document.createElement("div");
    const callRpc = vi.fn(async () => ({ plans: [plan()], worktrees: [worktree()] }));
    const navigate = vi.fn();
    const control = mountArchiveTab(host, { projectId: "project-1", callRpc, navigate, pollMs: 60_000 });

    await vi.waitFor(() => expect(host.querySelector("[data-plan]")).not.toBeNull());
    expect(callRpc).toHaveBeenCalledWith("archive.list", { project_id: "project-1" });
    host.querySelector("[data-plan]").click();
    expect(navigate).toHaveBeenCalledWith({
      name: "plan",
      projectId: "project-1",
      id: "plan-1",
      tab: "review",
    });
    control.dispose();
  });

  it("renders a calm retrying state when the archive is temporarily unavailable", async () => {
    const host = document.createElement("div");
    const control = mountArchiveTab(host, {
      projectId: "project-1",
      callRpc: async () => { throw new Error('<img src=x onerror="alert(1)">'); },
      navigate: () => {},
      pollMs: 60_000,
    });

    await vi.waitFor(() => expect(host.textContent).toContain("Archive is temporarily unavailable"));
    expect(host.innerHTML).not.toContain("<img");
    control.dispose();
  });

  it("stops retries and ignores in-flight responses after disposal", async () => {
    vi.useFakeTimers();
    const host = document.createElement("div");
    let resolveRequest;
    const callRpc = vi.fn(() => new Promise((resolve) => { resolveRequest = resolve; }));
    const control = mountArchiveTab(host, {
      projectId: "project-1",
      callRpc,
      navigate: () => {},
      pollMs: 1000,
    });
    const initialHtml = host.innerHTML;

    control.dispose();
    resolveRequest({ plans: [plan()], worktrees: [] });
    await vi.runAllTicks();
    await vi.advanceTimersByTimeAsync(5000);

    expect(host.innerHTML).toBe(initialHtml);
    expect(callRpc).toHaveBeenCalledTimes(1);
  });
});
