// @vitest-environment jsdom
// The workspace's rail, wired for real (#174): the cells core/directoryRail.js
// draws, the Issues count read off the project's cached issue list, and the
// settings sheet the cog opens — nothing between them stood in for. The
// workspace surface's own suite (workspaceViewDom) mounts it inside the view;
// this one proves the parts it hangs on the cells are the real ones.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, issue } from "./trackerWireFixture.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const { mountWorkspaceRail } = await import("../src/core/workspaceRail.js");
const { greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const { writeIssuesRecord } = await import("../src/core/trackerCache.js");
const { wipeCache } = await import("../src/core/localCache.js");
const { fakeSession } = await import("./deviceSessionFixture.js");

const PROJECT_KEY = "dev-1/p-1";
const HERE = "agent-01M2HERE";
const AWAY = "agent-01M2AWAY";
const HELLO = {
  api_version: "1.21.0",
  push_events: true,
  changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "issues"], items: "bodies" },
};

const route = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "ws-1", sourceId: "repo", tab: "changes" };
const workspace = { id: "ws-1", workspace_id: "ws-1", project_id: "p-1", name: "payment-work", projectKey: PROJECT_KEY, entity_id: "run-1" };
const feed = () => ({
  workspaces: [workspace],
  items: [{ kind: "branch", projectKey: PROJECT_KEY, run_id: "run-1", agents: [{ id: HERE, ordinal: 1 }] }],
});
const held = (agentId, over) => issue({ assignee: { kind: "agent", agent_id: agentId }, ...over });

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

let rail, selected, navigated;
const host = () => document.querySelector("#dir-rail");
const issuesCell = () => host().querySelector("[data-tab=issues]");

beforeEach(async () => {
  await wipeCache();
  document.body.innerHTML = bodyHtml;
  // The machine greets the way a real one does: its greeting installs the
  // adapter whose capabilities say it carries issues.
  await greetBridge(async () => HELLO, { deviceId: "dev-1" });
  adoptDeviceSession(fakeSession("dev-1"));
  selected = [];
  navigated = [];
  rail = mountWorkspaceRail(host(), {
    route,
    feed,
    workspace: () => workspace,
    onSelect: (tab) => selected.push(tab),
    navigate: (to) => navigated.push(to),
  });
});

afterEach(() => {
  rail.dispose();
  resetChangeEvents();
  resetDeviceContexts();
});

describe("the workspace rail, wired", () => {
  it("counts the open issues this workspace's agents hold on the Issues icon, and moves with the record", async () => {
    await writeIssuesRecord("dev-1", "p-1", {
      issues: [
        held(HERE, { number: 1, id: "i1", status: "in_progress" }),
        held(HERE, { number: 2, id: "i2", status: "done" }),
        held(AWAY, { number: 3, id: "i3", status: "in_progress" }),
      ],
      columns: columns(),
    });
    rail.paint("changes");
    await vi.waitFor(() => expect(issuesCell().querySelector(".dirtab-count").textContent).toBe("1"));
    expect(issuesCell().hidden).toBe(false);

    // A repaint draws a new cell; the count is on it at once.
    rail.paint("files");
    expect(issuesCell().querySelector(".dirtab-count").textContent).toBe("1");

    // An `issues` push rewrites the record; the icon follows.
    await writeIssuesRecord("dev-1", "p-1", {
      issues: [
        held(HERE, { number: 1, id: "i1", status: "in_progress" }),
        held(HERE, { number: 4, id: "i4", status: "ready" }),
      ],
      columns: columns(),
    });
    await vi.waitFor(() => expect(issuesCell().querySelector(".dirtab-count").textContent).toBe("2"));
  });

  it("hands a press on Issues to the surface, like any face", async () => {
    rail.paint("changes");
    issuesCell().click();
    expect(selected).toEqual(["issues"]);
  });

  it("opens the workspace's settings sheet from the cog at its foot", async () => {
    rail.paint("changes");
    host().querySelector("[data-rail-settings]").click();
    await flush();
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
    expect(document.getElementById("wslabel").value).toBe("payment-work");
    document.getElementById("wscancel").click();
    expect(selected).toEqual([]);
  });

  it("hands the shell's column back empty when the surface leaves", () => {
    rail.paint("changes");
    rail.dispose();
    expect(host().children).toHaveLength(0);
  });
});
