// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { deviceId, entityId, pinId, projectId, projectLineageFixture, spawnedProjectAliasFixture, topic, writeProjectLineageFixture } from "./inboxProjectLineageFixture.js";

let cache, feed, lineage, fixture, snapshot, model, stop;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  feed = await import("../src/core/taskFeed.js");
  model = await import("../src/core/inboxProjectAgent.js");
  fixture = projectLineageFixture();
  await writeProjectLineageFixture(cache, fixture);
  stop = feed.subscribeFeed((next) => { snapshot = next; });
  await feed.startFeed();
  const { mountAgentLineage } = await import("../src/core/agentLineage.js");
  lineage = mountAgentLineage({ deviceId, projectId });
  lineage.start();
  await vi.waitFor(() => expect(lineage.decorate(fixture.pin).agents_running).toBe(3));
});
afterEach(() => { lineage?.dispose(); stop?.(); feed?.stopFeed(); snapshot = null; });

const entry = () => model.projectAgentEntries(snapshot.projects, snapshot.items, snapshot.runs)[0];
const write = (kind, owner, value) => cache.writeCached({ deviceId, entityId: owner, kind }, value);

describe("the inbox project agent and chat share cached descendant activity (#697)", () => {
  it("counts the three workers shown by chat with a complete project roster", async () => {
    expect(lineage.decorate(fixture.pin)).toMatchObject({ working: false, agents_running: 3 });
    expect(entry()).toMatchObject({ name: topic, working: true, watchedWorking: true,
      facts: "3 running", unreadCount: 0 });
    expect((await cache.readCached({ deviceId, entityId, kind: "project-agents" })).value).toEqual(fixture.roster);
    expect((await cache.readCached({ deviceId, entityId: fixture.children[0].entity_id, kind: "row" })).value)
      .toEqual(fixture.children[0]);
  });

  it("repaints from a changed complete roster topic and child stop without a bridge refresh", async () => {
    await write("project-agents", entityId, { ...fixture.roster, agents: [{ ...fixture.pin, topic: "Review the inbox fix" }] });
    await vi.waitFor(() => expect(entry().name).toBe("Review the inbox fix"));
    const stopped = fixture.children[0];
    await write("row", stopped.entity_id, { ...stopped, agents: [{ ...stopped.agents[0], working: false }] });
    await vi.waitFor(() => expect(lineage.decorate(fixture.pin).agents_running).toBe(2));
    await vi.waitFor(() => expect(entry()).toMatchObject({ working: true, watchedWorking: true,
      facts: "2 running" }));
    for (const child of fixture.children.slice(1)) {
      await write("row", child.entity_id, { ...child, agents: [{ ...child.agents[0], working: false }] });
    }
    await vi.waitFor(() => expect(entry()).toMatchObject({ working: false, watchedWorking: false,
      facts: "0 running" }));
    expect(lineage.decorate(fixture.pin).agents_running || 0).toBe(0);
  });

  it("uses complete project workers as lineage members even when the legacy owner lists only its pin", async () => {
    const worker = { id: "project-worker", conversation_id: "project-worker-history", created_by: pinId,
      working: true, watched: false, unread_count: 0 };
    for (const child of fixture.children) {
      await write("row", child.entity_id, { ...child, agents: [{ ...child.agents[0], working: false }] });
    }
    await write("project-agents", entityId, { ...fixture.roster, agents: [worker, fixture.pin] });
    await vi.waitFor(() => expect(snapshot.projects[0].projectRoster.agents.some((agent) => agent.id === worker.id)).toBe(true));
    await vi.waitFor(() => expect(lineage.decorate(fixture.pin).agents_running).toBe(1));
    await vi.waitFor(() => expect(entry()).toMatchObject({ watchedWorking: true, working: true, facts: "1 running" }));
    expect(snapshot.projects[0].projectRoster.agents.find((agent) => agent.id === pinId))
      .toMatchObject({ working: false, agents_running: 1 });
  });

  it("counts an independent project worker once without inventing a creator for it", async () => {
    for (const child of fixture.children) {
      await write("row", child.entity_id, { ...child, agents: [{ ...child.agents[0], working: false }] });
    }
    await write("project-agents", entityId, { ...fixture.roster, agents: [fixture.pin, {
      id: "independent-worker", conversation_id: "independent-history", working: true, watched: false,
    }] });
    await vi.waitFor(() => expect(snapshot.projects[0].projectRoster.agents.some((agent) => agent.id === "independent-worker")).toBe(true));
    await vi.waitFor(() => expect(entry()).toMatchObject({ working: true, watchedWorking: false, facts: "1 running" }));
    expect(lineage.decorate(fixture.pin).agents_running || 0).toBe(0);
  });

  it("counts a canonical project conversation only once when the roster repeats it", async () => {
    for (const child of fixture.children) {
      await write("row", child.entity_id, { ...child, agents: [{ ...child.agents[0], working: false }] });
    }
    const worker = { id: "worker-copy-a", conversation_id: "same-worker-history", working: true, watched: false };
    await write("project-agents", entityId, { ...fixture.roster,
      agents: [fixture.pin, worker, { ...worker, id: "worker-copy-b" }],
    });
    await vi.waitFor(() => expect(snapshot.projects[0].projectRoster.agents).toHaveLength(3));
    await vi.waitFor(() => expect(entry().facts).toBe("1 running"));
  });

  it.each([false, true])("counts a spawned canonical child once in inbox and chat (exact repeat: %s)", async (repeated) => {
    for (const child of fixture.children) {
      await write("row", child.entity_id, { ...child, agents: [{ ...child.agents[0], working: false, created_by: null }] });
    }
    const next = spawnedProjectAliasFixture(repeated);
    await write("project-agents", entityId, next.roster);
    await vi.waitFor(() => expect(snapshot.projects[0].projectRoster.agents[1].conversation_id).toBe("worker-history"));
    await vi.waitFor(() => expect(lineage.buildAgentsFor(pinId)?.some((agent) => agent.id.startsWith("spawned-worker-"))).toBe(true));
    expect.soft(entry().facts).toBe("1 running");
    expect.soft(lineage.decorate(fixture.pin).agents_running).toBe(1);
    expect.soft(lineage.buildAgentsFor(pinId)).toHaveLength(1);
    expect((await cache.readCached({ deviceId, entityId, kind: "project-agents" })).value).toEqual(next.roster);
  });

  it("repaints chat when only a cached child's canonical conversation changes", async () => {
    for (const child of fixture.children) {
      await write("row", child.entity_id, { ...child, agents: [{ ...child.agents[0], working: false, created_by: null }] });
    }
    const next = spawnedProjectAliasFixture();
    const agents = next.roster.agents.map((agent, index) => index === 2
      ? { ...agent, conversation_id: "distinct-worker-history" } : agent);
    await write("project-agents", entityId, { ...next.roster, agents });
    await vi.waitFor(() => expect(lineage.buildAgentsFor(pinId)?.some((agent) => agent.id.startsWith("spawned-worker-"))).toBe(true));
    await vi.waitFor(() => expect(lineage.decorate(fixture.pin).agents_running).toBe(2));
    await write("project-agents", entityId, next.roster);
    await vi.waitFor(() => expect(lineage.decorate(fixture.pin).agents_running).toBe(1));
    expect(lineage.buildAgentsFor(pinId)).toHaveLength(1);
    await vi.waitFor(() => expect(entry().facts).toBe("1 running"));
  });

  it("counts the coordinator's own working loop alongside its three running panel entries", async () => {
    await write("project-agents", entityId, { ...fixture.roster, agents: [{ ...fixture.pin, working: true }] });
    await vi.waitFor(() => expect(entry()).toMatchObject({ working: true, watchedWorking: true, facts: "4 running" }));
  });

  it("counts a nested worker once as the coordinator's running panel entry", async () => {
    for (const child of fixture.children.slice(1)) {
      await write("row", child.entity_id, { ...child,
        agents: [{ ...child.agents[0], created_by: fixture.children[0].agents[0].id }],
      });
    }
    await vi.waitFor(() => expect(lineage.decorate(fixture.pin).agents_running).toBe(1));
    await vi.waitFor(() => expect(entry()).toMatchObject({ working: true, watchedWorking: true, facts: "1 running" }));
  });

  it.each([
    [{ stat: { uncommitted: { files_changed: 2, insertions: 3, deletions: 1 } } }, "2 files · +3 −1 · 3 running"],
    [{ pending: "updating" }, "Updating… · 3 running"],
  ])("preserves the owner's Git or pending facts alongside descendant activity: %j", async (fields, facts) => {
    await write("row", entityId, { ...fixture.owner, ...fields });
    await vi.waitFor(() => expect(entry().facts).toBe(facts));
  });

  it("does not roll up Build descendants when the cached createdBy capability is absent", async () => {
    await write("agent-lineage-support", "", { namesMakers: false });
    await vi.waitFor(() => expect(lineage.decorate(fixture.pin).agents_running || 0).toBe(0));
    await vi.waitFor(() => expect(entry()).toMatchObject({ working: false, watchedWorking: false,
      facts: "0 running" }));
  });

  it("shows zero running for an idle project with cached conversation history", async () => {
    for (const child of fixture.children) {
      await write("row", child.entity_id, { ...child, agents: [{ ...child.agents[0], working: false }] });
    }
    await cache.writeCached({ deviceId, entityId, kind: "thread", sub: fixture.pin.conversation_id }, {
      items: [{ type: "message", data: { sequence: 1, role: "user", body: "Review the project" } }],
      thread_total: 1, thread_last_sequence: 1,
    });
    await vi.waitFor(() => expect(entry().working).toBe(false));
    expect(entry().facts).toBe("0 running");
  });

  it("keeps Getting started for a truly empty conversation despite project-wide activity", async () => {
    for (const child of fixture.children) {
      await write("row", child.entity_id, { ...child, agents: [{ ...child.agents[0], working: false }] });
    }
    await write("row", entityId, { ...fixture.owner, session_started_ms: null, last_activity_ms: null,
      anchor: "2026-09-24T12:00:00Z", last_activity: "2026-09-24T12:00:00Z" });
    await write("projects", "", [{ ...fixture.project, session_started_ms: 100, last_activity_ms: 200 }]);
    await cache.writeCached({ deviceId, entityId, kind: "thread", sub: fixture.pin.conversation_id }, {
      items: [], thread_total: 0, thread_last_sequence: 0,
    });
    await vi.waitFor(() => expect(snapshot.items.find((row) => row.entity_id === entityId).session_started_ms).toBe(null));
    await vi.waitFor(() => expect(entry().working).toBe(false));
    expect(entry().facts).toBe("Getting started · 0 running");
  });

  it("recognizes legacy own conversation activity without modern session fields", async () => {
    for (const child of fixture.children) {
      await write("row", child.entity_id, { ...child, agents: [{ ...child.agents[0], working: false }] });
    }
    const owner = { ...fixture.owner, last_activity: "2026-09-24T12:00:00Z" };
    delete owner.session_started_ms;
    delete owner.last_activity_ms;
    await write("row", entityId, owner);
    await vi.waitFor(() => expect(snapshot.items.find((row) => row.entity_id === entityId).last_activity).toBe(owner.last_activity));
    await vi.waitFor(() => expect(entry().working).toBe(false));
    expect(entry().facts).toBe("0 running");
  });

});
