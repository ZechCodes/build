import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache;
let overview;
let scope;

const settle = async () => {
  for (let turn = 0; turn < 6; turn += 1) await new Promise((done) => setTimeout(done, 0));
};
const message = (sequence, role, body) => ({ type: "message", data: { sequence, role, body } });
const activity = (sequence, summary) => ({ type: "event", data: { sequence, event: "tool_use", summary } });
const rosterAddress = { deviceId: "dev-overview", entityId: "run-overview", kind: "row", sub: "" };
const threadAddress = (id) => ({ deviceId: "dev-overview", entityId: "run-overview", kind: "thread", sub: id });
const agent = (id, changes = {}) => ({ id, name: id, ordinal: 1, working: false,
  unread_count: 0, read_through_sequence: 0, ...changes });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  overview = await import("../src/core/agentOverview.js");
  scope = (await import("../src/core/cacheScope.js")).scopeFor("dev-overview");
});

describe("expanded agent overview", () => {
  it("prefers current work activity, then unread agent words, then the latest message", () => {
    const thread = { items: [message(1, "agent", "Earlier answer"),
      message(2, "user", "Please check this"), activity(3, "Running tests\nwith details"),
      message(4, "agent", "New answer")], activityDigests: [] };
    expect(overview.overviewSnippet(agent("A", { working: true, unread_count: 1 }), thread)).toBe("Running tests");
    expect(overview.overviewSnippet(agent("A", { unread_count: 1, read_through_sequence: 1 }), thread)).toBe("New answer");
    expect(overview.overviewSnippet(agent("A", { read_through_sequence: 4 }), thread)).toBe("New answer");
    expect(overview.overviewSnippet(agent("A"), { items: [message(1, "user", "My last question")] })).toBe("My last question");
  });

  it("mounts from cached roster and threads without a payload, then redraws on real cache writes", async () => {
    await cache.writeCached(rosterAddress, { kind: "branch", run_id: "run-overview", agents: [
      agent("A", { name: "Test agent", working: true }),
      agent("B", { name: "Review agent", unread_count: 1, read_through_sequence: 2 }),
    ] });
    await cache.writeCached(threadAddress("A"), { items: [activity(3, "Running tests")] });
    await cache.writeCached(threadAddress("B"), { items: [message(3, "agent", "Please review the diff")] });
    const paints = [];
    const reader = overview.createAgentOverview({
      scope,
      sources: () => [{ slot: "current", kind: "branch", entityId: "run-overview", address: rosterAddress }],
      onRows: (rows) => paints.push(rows),
    });
    reader.open();
    await settle();
    expect(paints.at(-1).map(({ name, snippet }) => [name, snippet])).toEqual([
      ["Test agent", "Running tests"], ["Review agent", "Please review the diff"],
    ]);

    await cache.writeCached(threadAddress("A"), { items: [activity(4, "Finishing lint")] });
    await settle();
    expect(paints.at(-1)[0].snippet).toBe("Finishing lint");

    await cache.writeCached(rosterAddress, { kind: "branch", run_id: "run-overview", agents: [agent("C", { name: "New agent" })] });
    await settle();
    expect(paints.at(-1).map((row) => row.name)).toEqual(["New agent"]);
    reader.close();
  });

  it("reads an issue agent's transcript through its cached execution context", async () => {
    const issueAddress = { deviceId: "dev-overview", entityId: "issue-1", kind: "issue", sub: "get" };
    await cache.writeCached(issueAddress, { issue_id: "issue-1", agents: [agent("issue-agent", { name: "Issue agent" })],
      execution_context: { entity_id: "run-implementation", agent_id: "worker-1", conversation_id: "conversation-1" } });
    await cache.writeCached({ deviceId: "dev-overview", entityId: "run-implementation", kind: "thread", sub: "conversation-1" },
      { items: [message(7, "agent", "Implementation ready")] });
    const paints = [];
    const reader = overview.createAgentOverview({ scope,
      sources: () => [{ slot: "current", kind: "issue", entityId: "issue-1", address: issueAddress }],
      onRows: (rows) => paints.push(rows),
    });
    reader.open();
    await settle();
    expect(paints.at(-1).map(({ name, snippet }) => [name, snippet]))
      .toEqual([["Issue agent", "Implementation ready"]]);
    reader.close();
  });
});
