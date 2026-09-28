/** @vitest-environment jsdom */
// #203: a machine's catalog is read once and kept, so when that machine's
// agent CLI is updated its bridge says `models.changed`, and the catalog is
// asked again — written to the cache, then heard by whoever draws it.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { createModelCatalog } from "../src/core/modelCatalog.js";
import { dispatchChangeEvent } from "../src/core/changeEvents.js";
import { readCached, wipeCache } from "../src/core/localCache.js";
import { deviceModelsAddress } from "../src/core/settingsRecords.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const offering = (version, models) => ({
  providers: [{ id: "claude_adk", label: "Claude Code", cli_name: "Claude Code", cli_version: version, models, efforts: [] }],
});
const OLD = offering("2.1.280", [{ id: "claude-opus-5-5", label: "Claude Opus 5.5" }]);
const NEW = offering("2.1.284", [
  { id: "claude-sonnet-5-5", label: "Claude Sonnet 5.5" },
  { id: "claude-opus-5-5", label: "Claude Opus 5.5" },
]);

let catalogs = [];
beforeEach(wipeCache);
afterEach(() => {
  for (const catalog of catalogs) catalog.disposeModelCatalog();
  catalogs = [];
});

function machine(deviceId, answers) {
  const context = { deviceId, active: () => true, rpc: vi.fn(async () => answers.shift()) };
  const catalog = createModelCatalog(context);
  catalogs.push(catalog);
  return { context, catalog };
}

it("asks a machine for its catalog again when its bridge says the models changed", async () => {
  const { context, catalog } = machine("dev-a", [OLD, NEW]);
  expect((await catalog.modelCatalog()).providers[0].cli_version).toBe("2.1.280");
  const heard = [];
  catalog.onModelCatalogChanged((held) => heard.push(held.providers[0].cli_version));

  expect(dispatchChangeEvent({ type: "models.changed" }, "dev-a")).toBe(true);

  await vi.waitFor(() => expect(heard).toContain("2.1.284"));
  expect(context.rpc).toHaveBeenCalledTimes(2);
  expect((await readCached(deviceModelsAddress("dev-a"))).value.providers[0].cli_version).toBe("2.1.284");
});

it("asks only the machine that said so", async () => {
  const a = machine("dev-a", [OLD, NEW]);
  const b = machine("dev-b", [OLD, NEW]);
  await a.catalog.modelCatalog();
  await b.catalog.modelCatalog();

  dispatchChangeEvent({ type: "models.changed" }, "dev-b");

  await vi.waitFor(() => expect(b.context.rpc).toHaveBeenCalledTimes(2));
  expect(a.context.rpc).toHaveBeenCalledTimes(1);
});

it("asks nothing of a machine whose catalog no surface has wanted", async () => {
  const { context } = machine("dev-a", [NEW]);

  dispatchChangeEvent({ type: "models.changed" }, "dev-a");
  await new Promise((done) => setTimeout(done, 20));

  expect(context.rpc).not.toHaveBeenCalled();
});

it("forgets a machine whose catalog was disposed", async () => {
  const { context, catalog } = machine("dev-a", [OLD, NEW]);
  await catalog.modelCatalog();
  catalog.disposeModelCatalog();

  expect(dispatchChangeEvent({ type: "models.changed" }, "dev-a")).toBe(true);
  await new Promise((done) => setTimeout(done, 20));

  expect(context.rpc).toHaveBeenCalledTimes(1);
});
