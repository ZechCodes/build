/** @vitest-environment jsdom */
// The picker over the real device registry, greeting and cache — nothing
// mocked but the bridge's own answers (#143). A refs read that failed on one
// session is retired by the next session's adoption: the status clears, and the
// new session is read once its greeting says it can be.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const scope = { workspace_id: "ws-1", source_id: "repo" };
const entityId = 'workspace:["ws-1","repo"]';
const listing = (name) => ({
  current: { kind: "branch", name, full_ref: `refs/heads/${name}` },
  refs: [{ kind: "branch", name, full_ref: `refs/heads/${name}`, current: true }],
});
const flush = async () => { for (let turn = 0; turn < 10; turn += 1) await new Promise((done) => setTimeout(done, 0)); };

let contexts;
let cache;
let mountWorkspaceRefPicker;
let mounted;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="host"></div>';
  contexts = await import("../src/core/deviceContexts.js");
  cache = await import("../src/core/localCache.js");
  ({ mountWorkspaceRefPicker } = await import("../src/core/workspaceRefPicker.js"));
  const { App } = await import("../src/app.js");
  App.devices = [{ id: "dev-a", name: "Laptop", status: "online" }];
});

afterEach(() => {
  mounted?.dispose();
  mounted = null;
  contexts.resetDeviceContexts();
});

const refsCalls = (call) => call.mock.calls.filter(([method]) => method === "git.refs");

function greet(context, call) {
  contexts.adoptDeviceSession({ deviceId: "dev-a", call });
  contexts.adoptBridgeSelection(context, { version: "2.0.0" }, null);
}

async function mountOver(context) {
  const host = document.getElementById("host");
  mounted = mountWorkspaceRefPicker(host, { scope, callRpc: context.rpc, cacheScope: context.cacheScope });
  await flush();
  return {
    label: () => host.querySelector(".workspace-reftrigger-name").textContent,
    status: () => host.querySelector('[role="status"]').textContent,
    toggle: () => host.querySelector("[data-refpicker-toggle]").click(),
  };
}

describe("workspace ref picker across a reconnect", () => {
  it.each([["untried"], ["blocked"]])("mounted while the machine is %s: quiet, then read by the adopted session", async (mode) => {
    const context = contexts.knownDeviceContext("dev-a");
    if (mode === "blocked") contexts.blockCurrentDevice("dev-a", "unreached");
    const picker = await mountOver(context);
    expect(picker.label()).toBe("Loading refs…");
    expect(picker.status()).toBe("");

    const call = vi.fn(async (method) => (method === "git.refs" ? listing("fresh") : { ok: true }));
    greet(context, call);
    await vi.waitFor(() => expect(picker.label()).toBe("fresh"));
    expect(refsCalls(call)).toEqual([["git.refs", scope]]);
    expect(picker.status()).toBe("");

    picker.toggle();
    await vi.waitFor(() => expect(refsCalls(call)).toHaveLength(2));
    await flush();
    expect(picker.status()).toBe("");
  });

  it("requested read error → greet → refresh leaves the status empty", async () => {
    const context = contexts.knownDeviceContext("dev-a");
    await cache.writeCached(context.cacheScope.address({ entityId, kind: "refs" }), listing("cached-main"));
    const refusing = vi.fn(async (method) => {
      if (method === "git.refs") throw new Error("Could not read refs: repository is locked");
      return { ok: true };
    });
    greet(context, refusing);
    const picker = await mountOver(context);
    expect(refsCalls(refusing)).toHaveLength(0);
    picker.toggle();
    await flush();
    expect(refsCalls(refusing)).toHaveLength(1);
    expect(picker.status()).toBe("Could not read refs: repository is locked");

    const call = vi.fn(async (method) => (method === "git.refs" ? listing("fresh") : { ok: true }));
    contexts.adoptDeviceSession({ deviceId: "dev-a", call });
    // The adoption itself retires the diagnostic; the read waits for the greeting.
    expect(picker.status()).toBe("");
    await flush();
    expect(refsCalls(call)).toHaveLength(0);
    contexts.adoptBridgeSelection(context, { version: "2.0.0" }, null);
    await vi.waitFor(() => expect(picker.label()).toBe("fresh"));
    expect(refsCalls(call)).toHaveLength(1);

    picker.toggle();
    picker.toggle();
    await vi.waitFor(() => expect(refsCalls(call)).toHaveLength(2));
    await flush();
    expect(picker.status()).toBe("");
  });

  it("a session adopted after a read that landed asks nothing", async () => {
    const context = contexts.knownDeviceContext("dev-a");
    const first = vi.fn(async (method) => (method === "git.refs" ? listing("main") : { ok: true }));
    greet(context, first);
    const picker = await mountOver(context);
    await vi.waitFor(() => expect(picker.label()).toBe("main"));

    const call = vi.fn(async () => ({ ok: true }));
    greet(context, call);
    await flush();
    expect(refsCalls(call)).toHaveLength(0);
    expect(picker.status()).toBe("");
  });
});
