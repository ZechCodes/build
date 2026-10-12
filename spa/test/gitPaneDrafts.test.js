// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { worktreeOf } from "./gitWireFixture.js";

let App, cache, uiStore, scopes, mountGitPane;
let mounted;
const tree = worktreeOf({ "src/a.js": "work in progress" });
const draftAddress = (deviceId) => ({ deviceId, entityId: "run-1", kind: "ui-draft", sub: "run:run-1" });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  sessionStorage.clear();
  document.body.innerHTML = "";
  mounted = [];
  ({ App } = await import("../src/appState.js"));
  cache = await import("../src/core/localCache.js");
  uiStore = await import("../src/core/localUiStore.js");
  scopes = await import("../src/core/cacheScope.js");
  ({ mountGitPane } = await import("../src/core/gitPane.js"));
});

afterEach(() => {
  mounted.forEach(({ pane }) => pane.dispose());
  scopes.clearCacheScope();
});

async function mount(deviceId, answers = {}) {
  await cache.writeCached({ deviceId, entityId: "run-1", kind: "status" }, tree.status());
  await cache.writeCached({ deviceId, entityId: "run-1", kind: "log" }, { commits: [], more: false });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const callRpc = async (method, params) => {
    if (answers[method]) return answers[method](params);
    return method === "git.diff" ? tree.diff(params) : {};
  };
  const pane = mountGitPane(container, { scope: { run_id: "run-1" }, cacheScope: scopes.scopeFor(deviceId), callRpc });
  const result = { pane, container };
  mounted.push(result);
  await vi.waitFor(() => expect(container.querySelector(".csinput")).not.toBeNull());
  return result;
}

function type({ container }, text) {
  const field = container.querySelector(".csinput");
  field.value = text;
  field.dispatchEvent(new window.Event("input", { bubbles: true }));
}

async function resetAccount() {
  App.accountEpoch += 1;
  scopes.clearCacheScope();
  await Promise.all([cache.wipeCache(), uiStore.wipeUiRecords()]);
}

describe("commit draft ownership", () => {
  it("isolates identical checkout IDs on different devices and restores each device's draft", async () => {
    const first = await mount("device-a");
    type(first, "Draft on machine A");
    const second = await mount("device-b");
    expect(second.container.querySelector(".csinput").value).toBe("");
    type(second, "Draft on machine B");
    first.pane.dispose();
    second.pane.dispose();
    const againA = await mount("device-a");
    const againB = await mount("device-b");
    expect(againA.container.querySelector(".csinput").value).toBe("Draft on machine A");
    expect(againB.container.querySelector(".csinput").value).toBe("Draft on machine B");
    await vi.waitFor(async () => expect((await uiStore.readUiRecord(draftAddress("device-a")))?.value.text).toBe("Draft on machine A"));
  });

  it.each([false, true])("leaves no prior-account draft after reset with a pending timer (late input: %s)", async (lateInput) => {
    const old = await mount("device-a");
    type(old, "Prior account's pending draft");
    await resetAccount();
    if (lateInput) type(old, "Late input from the old account");
    // Let the old writer's debounce settle before the new account remounts.
    await new Promise((resolve) => setTimeout(resolve, 250));
    const current = await mount("device-a");
    expect(current.container.querySelector(".csinput").value).toBe("");
    expect(await uiStore.readUiRecord(draftAddress("device-a"))).toBeUndefined();
  });

  it("cannot clear a new account's draft when the old account's composer completes late", async () => {
    let finishCommit;
    const commit = new Promise((resolve) => { finishCommit = resolve; });
    const sendCommit = vi.fn(() => commit);
    const old = await mount("device-a", { "git.commit": sendCommit });
    type(old, "Old account's commit");
    old.container.querySelector('.csbox-actions [data-action="commit"]').click();
    await vi.waitFor(() => expect(sendCommit).toHaveBeenCalledOnce());
    await resetAccount();
    const current = await mount("device-a");
    type(current, "New account's draft");
    finishCommit({ status: tree.status() });
    await vi.waitFor(async () => expect((await uiStore.readUiRecord(draftAddress("device-a")))?.value.text).toBe("New account's draft"));
    const remounted = await mount("device-a");
    expect(remounted.container.querySelector(".csinput").value).toBe("New account's draft");
  });

  it("does not replay a page-exit journal from the previous account after a real application reset", async () => {
    const { resetApplication } = await import("../src/app.js");
    const old = await mount("device-a");
    type(old, "Prior account's page-exit draft");
    window.dispatchEvent(new window.Event("pagehide"));
    const journalKey = `build.ui.pending:${JSON.stringify(["device-a", "run-1", "ui-draft", "run:run-1"])}`;
    expect(sessionStorage.getItem(journalKey)).toContain("Prior account's page-exit draft");
    resetApplication();
    await vi.waitFor(async () => expect(await uiStore.readUiRecord(draftAddress("device-a"))).toBeUndefined());
    expect(sessionStorage.getItem(journalKey)).toBeNull();
    type(old, "A retired pane cannot journal again");
    window.dispatchEvent(new window.Event("pagehide"));
    expect(sessionStorage.getItem(journalKey)).toBeNull();
    const current = await mount("device-a");
    expect(current.container.querySelector(".csinput").value).toBe("");
    expect(await uiStore.readUiRecord(draftAddress("device-a"))).toBeUndefined();
  });
});
