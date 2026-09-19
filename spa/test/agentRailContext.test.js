// The rail's adapters: how each kind of work item is named, and the one read
// the rail still makes — the page above a window the reader has scrolled to the
// top of, written into the record the panel paints from.

import { describe, expect, it, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, agentRailContext;
let createAgentRailContext, widenCachedThread;

const address = { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "agent-2" };
const item = (sequence) => ({ type: "message", data: { sequence, role: "agent", body: `m-${sequence}` } });
const digest = (from, through) => ({
  from_sequence: from,
  through_sequence: through,
  tool_calls: 3,
  rows: 3,
  last_tool_call: null,
});

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  agentRailContext = await import("../src/core/agentRailContext.js");
  ({ createAgentRailContext, widenCachedThread } = agentRailContext);
});

describe("agent rail context adapters", () => {
  // Nothing here reads a work item the board writes a row for: its agents and
  // its conversation are in the cache, and the rail paints them from there.
  it("offers no read of the work item where the board writes a row", () => {
    const kinds = [
      { kind: "branch", projectId: "p1", branch: "build/chat" },
      { kind: "project", projectId: "p1", entityId: "run-9" },
      { kind: "workspace", projectId: "p1", workspaceId: "ws-1" },
    ];
    for (const context of kinds) {
      expect(createAgentRailContext(context).detail).toBeUndefined();
      expect(createAgentRailContext(context).workItem).toBeUndefined();
    }
  });

  // Except an issue, which left the board: nothing pushes one a row and the
  // sync layer's ordered pass never walks one, so `issue.get` is the only
  // thing on either side that says who its agents are.
  it("reads an issue's own work item, because the board writes it none", async () => {
    const call = vi.fn(async () => ({ issue_id: "issue-1", agents: [] }));
    const issue = createAgentRailContext({ kind: "issue", projectId: "p1", issueId: "issue-1" });

    expect(await issue.workItem(call)).toEqual({ issue_id: "issue-1", agents: [] });
    expect(call).toHaveBeenCalledWith("issue.get", { issue_id: "issue-1" });
    expect(issue.detail).toBeUndefined();
  });

  it("names a branch by the route its row is found under", () => {
    const context = createAgentRailContext({ kind: "branch", deviceId: "dev-2", projectId: "p1", branch: "build/chat" });

    expect(context.key).toBe("branch:p1:build/chat");
    // The row this rail is about is on one machine: the route it looks for
    // carries the device, the way every other route does.
    expect(context.feedRoute()).toEqual({ name: "branch", deviceId: "dev-2", projectId: "p1", branch: "build/chat" });
  });

  it("names an issue and a workspace the way their routes do", () => {
    const issue = createAgentRailContext({ kind: "issue", deviceId: "dev-2", projectId: "p1", issueId: "issue-1" });
    expect(issue.key).toBe("issue:issue-1");
    expect(issue.feedRoute()).toEqual({ name: "issue", deviceId: "dev-2", projectId: "p1", id: "issue-1" });

    const workspace = createAgentRailContext({
      kind: "workspace", deviceId: "dev-2", projectId: "p1", workspaceId: "run-1",
    });
    expect(workspace.key).toBe("workspace:run-1");
    // A workspace is named across the account by its machine and its id
    // together: the route this rail looks itself up by carries the machine, the
    // way the branch and issue routes beside it do.
    expect(workspace.feedRoute()).toEqual({
      name: "workspace", deviceId: "dev-2", projectId: "p1", workspaceId: "run-1",
    });
  });

  // A project is a conversation owner the way a workspace is: the page mints
  // the owner before it mounts the rail, and the rail reads that owner's row.
  it("carries the entity a project's page was handed", () => {
    const context = createAgentRailContext({
      kind: "project", deviceId: "dev-2", projectId: "p1", entityId: "run-9",
    });

    expect(context.entityId).toBe("run-9");
    expect(context.key).toBe("project:dev-2/p1");
    expect(context.feedRoute()).toEqual({ name: "project", deviceId: "dev-2", projectId: "p1" });
  });

  it("creates conversation ownership only where there is an owner to mint", async () => {
    const call = vi.fn(async () => ({ entity_id: "run-1" }));
    const workspace = createAgentRailContext({ kind: "workspace", projectId: "p1", workspaceId: "workspace-1" });
    const project = createAgentRailContext({ kind: "project", projectId: "p1", entityId: "run-9" });
    const branch = createAgentRailContext({ kind: "branch", projectId: "p1", branch: "build/chat" });
    const issue = createAgentRailContext({ kind: "issue", projectId: "p1", issueId: "issue-1" });

    expect(await workspace.ensureConversation(call)).toEqual({ entity_id: "run-1" });
    expect(await project.ensureConversation(call)).toEqual({ entity_id: "run-1" });
    expect(branch.ensureConversation(call)).toBeNull();
    expect(issue.ensureConversation(call)).toBeNull();
    expect(call).toHaveBeenCalledTimes(2);
    expect(call).toHaveBeenNthCalledWith(1, "workspace.ensure_conversation", { workspace_id: "workspace-1" });
    expect(call).toHaveBeenNthCalledWith(2, "project.ensure_conversation", { project_id: "p1" });
  });
});

describe("the page above the window", () => {
  const page = { items: [item(1), item(2)], activity_digests: [digest(1, 2)], has_more: false };

  it("asks for the history before the seek and writes it into the record", async () => {
    await cache.writeCached(address, {
      items: [item(3), item(4)],
      deliveredSequence: 4,
      olderItemsRemain: true,
      activityDigests: [digest(3, 4)],
    });
    const call = vi.fn(async () => page);
    const context = createAgentRailContext({ kind: "branch", deviceId: "dev-1", projectId: "p1", branch: "b" });

    const answered = await context.olderPage(call, {
      entityId: "run-1", agentId: "agent-2", beforeSequence: 3, address,
    });

    expect(answered).toBe(page);
    expect(call).toHaveBeenCalledWith("thread.page", {
      entity_id: "run-1", agent_id: "agent-2", before_sequence: 3,
    });
    const record = (await cache.readCached(address)).value;
    expect(record.items.map((held) => held.data.sequence)).toEqual([1, 2, 3, 4]);
    // What the page says about the far end is the answer about the floor this
    // window now has, and it replaces what the last page said.
    expect(record.olderItemsRemain).toBe(false);
    expect(record.activityDigests.map((held) => held.from_sequence)).toEqual([1, 3]);
  });

  // A round trip is long enough for the window to be replaced under it — the
  // reader switched agents, or a fresh window opened on the newest items.
  // Folding the page in then would seat it under a floor it was never below,
  // with everything in between missing and nothing to ask for it again.
  it("drops a page whose window moved while it was in flight", async () => {
    const held = { items: [item(9)], deliveredSequence: 9, olderItemsRemain: true, activityDigests: [] };
    await cache.writeCached(address, held);

    await widenCachedThread(address, page, 3);

    expect((await cache.readCached(address)).value).toEqual(held);
  });

  it("leaves a record that holds no window at all alone", async () => {
    await widenCachedThread(address, page, 3);
    expect(await cache.readCached(address)).toBeUndefined();
  });
});
