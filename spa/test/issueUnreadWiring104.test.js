/** @vitest-environment jsdom */
// #104, unmocked: the real Issues pane over the real cache. The remote RPC is
// held open, so every bubble on screen has to come from cached records — the
// list's `unread_count` first, then a cached timeline that says it was read.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountIssuesPane } from "../src/core/trackerIssuesPane.js";
import { resetChangeEvents } from "../src/core/changeEvents.js";
import { issueRecord, issuesRecord, writeIssueRecord, writeIssuesRecord } from "../src/core/trackerCache.js";
import { columns, comment, issue } from "./trackerWireFixture.js";

const DEVICE = "wiring-device";
let pane;
let host;

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="issues"></div>';
  host = document.querySelector("#issues");
});

afterEach(() => {
  pane?.dispose();
  pane = null;
  resetChangeEvents();
});

const mount = (projectId, view) => {
  pane = mountIssuesPane(host, {
    deviceId: DEVICE,
    projectId,
    projectName: "Build",
    projectKey: projectId,
    view,
    callRpc: () => new Promise(() => {}),
    catalog: () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }),
  });
};

const bubbleOf = (issueId) => host.querySelector(`[data-issue="${issueId}"] .issue-unread`)?.textContent ?? null;

describe.each(["list", "board", "dashboard"])("the %s", (view) => {
  const projectId = `wiring-${view}`;
  const asked = issue({
    id: "asked", number: 4, title: "Look at this", status: "ready", assignee: { kind: "user" },
    watched: true, read_through: "ic-01", unread_count: 2,
  });
  const quiet = issue({ id: "quiet", number: 5, title: "Nobody watches", status: "ready", assignee: { kind: "user" }, unread_count: 7 });

  it("wears the bubble a watched issue's cached list row says, and loses it once a cached read says so", async () => {
    await writeIssuesRecord(DEVICE, projectId, issuesRecord([asked, quiet], columns()));
    mount(projectId, view);
    await vi.waitFor(() => expect(bubbleOf("asked")).toBe("2"));
    expect(host.querySelector('[data-issue="quiet"]')).not.toBeNull();
    expect(bubbleOf("quiet")).toBeNull();

    const timeline = [
      comment({ id: "ic-02", author: { kind: "agent", agent_id: "a1" } }),
      comment({ id: "ic-03", author: { kind: "agent", agent_id: "a1" } }),
    ];
    await writeIssueRecord(DEVICE, projectId, "asked", issueRecord({ ...asked, read_through: "ic-03" }, timeline));
    await vi.waitFor(() => expect(bubbleOf("asked")).toBeNull());
  });
});
