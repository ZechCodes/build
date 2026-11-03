// @vitest-environment jsdom
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { pagedAnswer, patchFor, worktreeOf } from "./gitWireFixture.js";

const HASH = "a".repeat(40);
const PATH = "src/target.js";
const tree = worktreeOf();
const status = tree.status();
const log = { commits: [{ hash: HASH, short: HASH.slice(0, 8), subject: "Change target", author: "Ada", time: 1 }], more: false };
const show = (patch) => ({ ...log.commits[0], body: "", patch, truncated: false, stat: { files_changed: 1, insertions: 1, deletions: 1 } });
let cache, events, scopeFor, mountGitPane;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = "";
  cache = await import("../src/core/localCache.js");
  events = await import("../src/core/changeEvents.js");
  ({ scopeFor } = await import("../src/core/cacheScope.js"));
  ({ mountGitPane } = await import("../src/core/gitPane.js"));
  await cache.writeCached({ deviceId: "d", entityId: "run", kind: "status" }, status);
  await cache.writeCached({ deviceId: "d", entityId: "run", kind: "log" }, log);
});
afterEach(() => events.resetChangeEvents());
const address = (sub) => ({ deviceId: "d", entityId: "run", kind: "patch", sub });
function mount(callRpc, requestedDiff = { path: PATH, side: "old", line: 1 }) {
  const host = document.createElement("div");
  document.body.append(host);
  const pane = mountGitPane(host, { scope: { run_id: "run" }, cacheScope: scopeFor("d"), callRpc,
    requestedCommit: HASH.slice(0, 8), requestedDiff });
  return { host, pane };
}

describe("workspace commit diff links", () => {
  it("opens cached old-side lines without a connection", async () => {
    await cache.writeCached(address(HASH), show(patchFor(PATH, "new")));
    const call = vi.fn(() => new Promise(() => {}));
    const { host, pane } = mount(call);
    await vi.waitFor(() => expect(host.querySelector("tr.diff-link-anchor")?.dataset.oldLine).toBe("1"));
    expect(call).not.toHaveBeenCalled();
    expect(host.querySelector(".file").classList.contains("capped")).toBe(false);
    pane.dispose();
  });
  it("lands beyond the first virtual row window and inside generated files", async () => {
    const path = "package-lock.json";
    await cache.writeCached(address(HASH), show(patchFor(path, Array.from({ length: 2000 }, (_, i) => `row ${i + 1}`))));
    const { host, pane } = mount(vi.fn(() => new Promise(() => {})), { path, side: "new", line: 1800 });
    await vi.waitFor(() => expect(host.querySelector("tr.diff-link-anchor")?.dataset.newLine).toBe("1800"));
    expect(host.querySelectorAll("tr[aria-rowindex]").length).toBeLessThan(300);
    pane.dispose();
  });
  it("filters reads only with git.showPaths and caches them separately from whole commits", async () => {
    await events.greetBridge(async () => ({ api_version: "3.28.0", capabilities: ["git.showPaths"] }), { deviceId: "d" });
    const call = vi.fn(async (method) => method === "git.show" ? show(patchFor(PATH, "new")) : {});
    const { host, pane } = mount(call);
    await vi.waitFor(() => expect(host.querySelector("tr.diff-link-anchor")).not.toBeNull());
    expect(call).toHaveBeenCalledWith("git.show", { run_id: "run", hash: HASH, paths: [PATH] });
    expect(await cache.readCached(address(HASH))).toBeUndefined();
    pane.dispose();
    const offline = vi.fn(() => new Promise(() => {}));
    const next = mount(offline);
    await vi.waitFor(() => expect(next.host.querySelector("tr.diff-link-anchor")).not.toBeNull());
    expect(offline).not.toHaveBeenCalled();
    next.pane.dispose();
  });
  it("uses a filtered body beside an older whole-commit header record", async () => {
    await cache.writeCached(address(HASH), { ...show(""), truncated: true });
    await events.greetBridge(async () => ({ api_version: "3.28.0", capabilities: ["git.showPaths"] }), { deviceId: "d" });
    const call = vi.fn(async () => show(patchFor(PATH, "new")));
    const { host, pane } = mount(call);
    await vi.waitFor(() => expect(host.querySelector("tr.diff-link-anchor")).not.toBeNull());
    expect((await cache.readCached(address(HASH))).value.truncated).toBe(true);
    pane.dispose();
  });
  it("seeks a linked line through pages while preserving its path filter", async () => {
    await events.greetBridge(async () => ({ api_version: "3.28.0", capabilities: ["git.showPaths", "bodies.pages"] }), { deviceId: "d" });
    const whole = patchFor(PATH, Array.from({ length: 300 }, (_, i) => `line ${i + 1}`));
    const call = vi.fn(async (_method, params) => params.range
      ? { ...show(""), ...pagedAnswer(whole, params.range.offset, { pageBytes: 500 }) }
      : { ...show(whole.slice(0, 200)), truncated: true, patch_bytes: whole.length });
    const { host, pane } = mount(call, { path: PATH, side: "new", line: 280 });
    await vi.waitFor(() => expect(host.querySelector("tr.diff-link-anchor")?.dataset.newLine).toBe("280"), { timeout: 5000 });
    expect(call.mock.calls.length).toBeGreaterThan(3);
    expect(call.mock.calls.every(([, params]) => params.paths?.[0] === PATH)).toBe(true);
    pane.dispose();
  });
  it("falls back to a whole read when a cached filtered page outlives showPaths support", async () => {
    const whole = patchFor(PATH, Array.from({ length: 300 }, (_, i) => `line ${i + 1}`));
    const key = JSON.stringify([HASH, PATH]);
    const pages = await import("../src/core/bodyPages.js");
    const first = pagedAnswer(whole, 0, { pageBytes: 500 });
    await pages.writeBodyPage(address(key), pages.pageFromAnswer(first, "patch"));
    const head = { ...show(""), paged: true, of: "v1", truncated: true };
    delete head.patch;
    await cache.writeCached(address(key), head);
    await events.greetBridge(async () => ({ api_version: "3.28.0", capabilities: ["bodies.pages"] }), { deviceId: "d" });
    const call = vi.fn(async () => show(whole));
    const { host, pane } = mount(call, { path: PATH, side: "new", line: 280 });
    await vi.waitFor(() => expect(host.querySelector("tr.diff-link-anchor")?.dataset.newLine).toBe("280"));
    expect(call).toHaveBeenCalledWith("git.show", { run_id: "run", hash: HASH });
    expect(call.mock.calls.every(([, params]) => params.paths === undefined)).toBe(true);
    pane.dispose();
  });
  it("falls back to the whole commit when git.showPaths is absent", async () => {
    await events.greetBridge(async () => ({ api_version: "3.28.0", capabilities: [] }), { deviceId: "d" });
    const call = vi.fn(async () => show(patchFor("src/other.js", "other") + patchFor(PATH, "new")));
    const { host, pane } = mount(call);
    await vi.waitFor(() => expect(host.querySelector("tr.diff-link-anchor")).not.toBeNull());
    expect(call).toHaveBeenCalledWith("git.show", { run_id: "run", hash: HASH });
    expect((await cache.readCached(address(HASH))).value.patch).toContain("src/other.js");
    pane.dispose();
  });
});
