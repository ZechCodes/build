/** @vitest-environment jsdom */
// #221, with nothing mocked between the greeting and the reader: the bridge's
// greeting (fixtures/api/v1/session.hello.json, which bridge/tests pins to the
// real reply) names `agents.createdBy`; the real greeting path writes it to
// the real cache; the mounted lineage reader reads it there and lists the
// Build agents a row names as made by an agent. A greeting without the name
// leaves a reader that lists none, whatever the rows carry.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { mountAgentLineage } = await import("../src/core/agentLineage.js");
const { readAgentLineageSupport } = await import("../src/core/agentLineageSupport.js");
const { writeCached } = await import("../src/core/localCache.js");

const greeting = JSON.parse(readFileSync(resolve(process.cwd(), "../fixtures/api/v1/session.hello.json"), "utf8")).result;
const olderGreeting = { ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "agents.createdBy") };

let reader = null;

beforeEach(async () => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
  await writeCached({ deviceId: "dev-1", entityId: "run-a", kind: "row" }, {
    kind: "workspace", run_id: "run-a", project_id: "proj-1", workspace_id: "ws-a",
    agents: [{ id: "worker", created_by: "boss" }],
  });
});

afterEach(() => {
  reader?.dispose();
  reader = null;
  resetChangeEvents();
});

const greet = (hello) => greetBridge(async (method) => (method === "session.hello" ? hello : {}), { deviceId: "dev-1", strict: true });

it("lists the Build agents once the bridge's real greeting names agents.createdBy", async () => {
  expect(greeting.capabilities).toContain("agents.createdBy");
  await greet(greeting);
  await vi.waitFor(async () => expect(await readAgentLineageSupport("dev-1")).toBe(true));
  reader = mountAgentLineage({ deviceId: "dev-1", projectId: "proj-1", onChanged: () => {} });
  reader.start();
  await vi.waitFor(() => expect(reader.buildAgentsFor("boss")).toHaveLength(1));
});

it("lists none for a bridge whose greeting does not name it", async () => {
  await greet(olderGreeting);
  let changed = 0;
  reader = mountAgentLineage({ deviceId: "dev-1", projectId: "proj-1", onChanged: () => changed++ });
  reader.start();
  await vi.waitFor(() => expect(changed).toBeGreaterThan(0));
  expect(await readAgentLineageSupport("dev-1")).toBe(false);
  expect(reader.buildAgentsFor("boss")).toBeNull();
});
