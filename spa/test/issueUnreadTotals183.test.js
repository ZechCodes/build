/** @vitest-environment jsdom */
// #183, unmocked: a Done issue's unread never goes into a total, whatever a
// 1.29 list says — not the project's Issues tab, not the workspace's Issues
// face or its tooltip — while the issue's own bubble still shows it. The real
// cache, the real followers and the real Issues pane; the remote RPC never
// answers, so everything on screen came from the cached list.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountIssuesPane } from "../src/core/trackerIssuesPane.js";
import { followProjectIssuesUnread } from "../src/core/projectIssuesUnread.js";
import { mountWorkspaceIssues } from "../src/core/trackerWorkspaceIssuesView.js";
import { resetChangeEvents } from "../src/core/changeEvents.js";
import { issuesRecord, writeIssuesRecord } from "../src/core/trackerCache.js";
import { columns, issue } from "./trackerWireFixture.js";

const DEVICE = "totals-183-device";
const HOLDER = "agent-01M3HOLDER";
let disposers = [];

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<button data-tab="issues" hidden><span class="badge dirtab-count"></span></button><div id="issues"></div>';
});

afterEach(() => {
  disposers.forEach((dispose) => dispose());
  disposers = [];
  resetChangeEvents();
});

// The issue holds 1 unread while it is being worked, then goes to Done with
// the 803 a 1.29 bridge counts on it.
const working = issue({ id: "i-50", number: 50, title: "Finished work", status: "in_progress", watched: true,
  unread_count: 1, assignee: { kind: "agent", agent_id: HOLDER } });
const finished = { ...working, status: "done", unread_count: 803, updated_at: "2026-08-22T10:00:00Z" };
const list = (projectId, one) => writeIssuesRecord(DEVICE, projectId, issuesRecord([one], columns()));

describe("a Done issue carrying unread_count", () => {
  it("counts nothing on the project's Issues tab", async () => {
    const tab = followProjectIssuesUnread();
    disposers.push(tab.dispose);
    await list("project-tab", working);
    tab.follow(DEVICE, "project-tab");
    await vi.waitFor(() => expect(tab.count()).toBe(1));
    await list("project-tab", finished);
    await vi.waitFor(() => expect(tab.count()).toBe(0));
  });

  it("counts nothing on the workspace's Issues face, nor in its tooltip", async () => {
    const button = document.querySelector("[data-tab=issues]");
    const badge = () => button.querySelector(".dirtab-count").textContent;
    await list("workspace-face", working);
    const face = mountWorkspaceIssues(button, {
      deviceId: DEVICE, projectId: "workspace-face", workspaceId: "ws-1", agents: () => [{ id: HOLDER, ordinal: 1 }],
    });
    disposers.push(() => face.dispose());
    await vi.waitFor(() => expect(badge()).toBe("1"));
    expect(button.title).toBe("1 unread · 1 open issue in this workspace");
    await list("workspace-face", finished);
    await vi.waitFor(() => expect(badge()).toBe(""));
    expect(button.title).toBe("Issues in this workspace");
  });

  it.each(["list", "board"])("still wears its own bubble on the %s", async (view) => {
    const projectId = `pane-${view}`;
    await list(projectId, finished);
    const host = document.querySelector("#issues");
    const pane = mountIssuesPane(host, {
      deviceId: DEVICE, projectId, projectName: "Build", projectKey: projectId, view,
      callRpc: () => new Promise(() => {}), catalog: () => ({ providers: [] }), feed: () => ({ items: [], workspaces: [] }),
    });
    disposers.push(() => pane.dispose());
    await vi.waitFor(() => expect(host.querySelector('[data-issue="i-50"] .issue-unread')?.textContent).toBe("803"));
  });
});
