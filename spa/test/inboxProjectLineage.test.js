// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { deviceId, entityId, pinId, projectId, projectLineageFixture, topic, writeProjectLineageFixture } from "./inboxProjectLineageFixture.js";

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
      facts: "Getting started · 0 running" }));
    expect(lineage.decorate(fixture.pin).agents_running || 0).toBe(0);
  });

  it("uses complete project workers as lineage members even when the legacy owner lists only its pin", async () => {
    const worker = { id: "project-worker", conversation_id: "project-worker-history", created_by: pinId,
      working: true, watched: false, unread_count: 0 };
    for (const child of fixture.children) {
      await write("row", child.entity_id, { ...child, agents: [{ ...child.agents[0], working: false }] });
    }
    await write("project-agents", entityId, { ...fixture.roster, agents: [worker, fixture.pin] });
    await vi.waitFor(() => expect(lineage.decorate(fixture.pin).agents_running).toBe(1));
    await vi.waitFor(() => expect(entry()).toMatchObject({ watchedWorking: true, working: true }));
    expect(snapshot.projects[0].projectRoster.agents.find((agent) => agent.id === pinId))
      .toMatchObject({ working: false, agents_running: 1 });
  });

  it("does not roll up Build descendants when the cached createdBy capability is absent", async () => {
    await write("agent-lineage-support", "", { namesMakers: false });
    await vi.waitFor(() => expect(lineage.decorate(fixture.pin).agents_running || 0).toBe(0));
    await vi.waitFor(() => expect(entry()).toMatchObject({ working: false, watchedWorking: false,
      facts: "Getting started · 0 running" }));
  });

});
