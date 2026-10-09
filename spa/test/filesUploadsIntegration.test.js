// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { untilDom } from "./untilCondition.js";
vi.mock("../src/core/fileUploadRpc.js", () => ({ fileUploadRpc: (_context, callRpc) => callRpc }));
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { scopeFor } = await import("../src/core/cacheScope.js");
const { readCached, wipeCache } = await import("../src/core/localCache.js");
const { wipeUiRecords } = await import("../src/core/localUiStore.js");
const { rememberFileUploadSupport } = await import("../src/core/fileUploadSupport.js");
const { uploadsFor } = await import("../src/core/fileUploads.js");
const { cacheFileBody } = await import("../src/core/cacheLifetime.js");
const { renderFilesTab } = await import("../src/views/files.js");
let mounted = [], engines = [], device;
beforeEach(async () => {
  document.body.innerHTML = "";
  device = crypto.randomUUID();
  await wipeCache(); await wipeUiRecords();
  await rememberFileUploadSupport(device, { fs: { uploads: true, createDirectory: true } });
});
afterEach(() => { mounted.forEach((view) => view.dispose()); engines.forEach((engine) => engine.dispose()); mounted = []; engines = []; });
const sources = [
  { label: "workspace source", scope: { workspace_id: "w", source_id: "code" }, rooted: true },
  { label: "project source", scope: { project_id: "p", source_id: "code" }, rooted: true },
  { label: "checkout", scope: { run_id: "r" }, rooted: false },
];
const mount = (source, callRpc) => {
  const host = document.createElement("div"); document.body.append(host);
  const options = source.rooted ? { roots: [{ id: "code", label: "Code", scope: source.scope }], layoutEntityId: "layout" } : { scope: source.scope };
  const view = renderFilesTab(host, { ...options, callRpc, cacheScope: scopeFor(device) });
  mounted.push(view);
  return { host, view };
};
const fakeRpc = () => {
  const listings = { "": [{ name: "ignored", kind: "dir" }], ignored: [] };
  const bodies = {};
  const heldReads = {};
  let file;
  const callRpc = vi.fn(async (method, params) => {
    if (method === "fs.tree") return { path: params.path, entries: [...(listings[params.path] || [])] };
    if (method === "fs.createDirectory") { listings[params.parent].push({ name: params.name, kind: "dir" }); return { path: `${params.parent}/${params.name}` }; }
    if (method === "fs.createFile") { listings[params.parent].push({ name: params.name, kind: "file", size: 0 }); return { path: `${params.parent}/${params.name}` }; }
    if (method === "fs.read" && heldReads[params.path]) {
      const held = heldReads[params.path]; delete heldReads[params.path];
      return held();
    }
    if (method === "fs.read") {
      const body = bodies[params.path] || "";
      return { path: params.path, size: body.length, truncated: false, mime: "text/plain", content_b64: btoa(body), editable: true, encoding: "utf-8", revision: body ? "old" : "e3b0c442" };
    }
    if (method === "fs.uploadBegin") { file = params; return { upload_id: "u", path: `${params.parent}/${params.name}`, chunk_bytes: 2 }; }
    if (method === "fs.uploadChunk") return { received: params.offset + atob(params.content_b64).length };
    if (method === "fs.uploadFinish") { listings[file.parent].push({ name: file.name, kind: "file", size: file.size }); return { path: `${file.parent}/${file.name}`, size: file.size }; }
    throw new Error(`Unexpected ${method}`);
  });
  return Object.assign(callRpc, { listings, bodies, heldReads });
};
const chooseFiles = (host) => {
  host.querySelector('[data-path="ignored"] [data-upload-action="upload"]').click();
  const input = host.querySelector('input[type="file"]');
  Object.defineProperty(input, "files", { value: [new File(["abc"], "new.txt")] });
  input.dispatchEvent(new Event("change"));
};

for (const source of sources) {
  it(`refreshes and reveals an ignored upload and new folder in a ${source.label} without a push`, async () => {
    const rpc = fakeRpc();
    const { host } = mount(source, rpc);
    const engine = uploadsFor(device); engines.push(engine);
    await vi.waitFor(() => expect(host.querySelector('[data-path="ignored"] [data-upload-action="upload"]')).toBeTruthy());
    expect(host.querySelector('[data-path="ignored"]').getAttribute("aria-expanded")).toBe("false");
    chooseFiles(host);
    await vi.waitFor(() => expect(host.querySelector('[data-path="ignored/new.txt"]')).toBeTruthy());
    expect(rpc.mock.calls.find(([method]) => method === "fs.uploadBegin")[1]).toMatchObject({ ...source.scope, parent: "ignored", name: "new.txt", size: 3 });
    expect(host.querySelector(".fupload-tray").hidden).toBe(false);
    const reads = rpc.mock.calls.filter(([method, params]) => method === "fs.tree" && params.path === "ignored").length;
    host.querySelector('[data-path="ignored"] [data-upload-action="folder"]').click();
    const input = host.querySelector(".fupload-folder input"); input.value = "drafts";
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(host.querySelector('[data-path="ignored/drafts"]')).toBeTruthy());
    expect(rpc.mock.calls.filter(([method, params]) => method === "fs.tree" && params.path === "ignored").length).toBeGreaterThan(reads);
  });
}
for (const source of sources) {
  it(`creates a file from the New menu in a ${source.label}, then selects and opens it`, async () => {
    await rememberFileUploadSupport(device, { fs: { uploads: true, createDirectory: true, createFile: true } });
    const rpc = fakeRpc();
    const { host } = mount(source, rpc);
    await vi.waitFor(() => expect(host.querySelector('[data-path="ignored"] .fupload-new .caret')).toBeTruthy());
    host.querySelector('[data-path="ignored"] .fupload-new .caret').click();
    host.querySelector('[data-path="ignored"] .fupload-new [data-action="file"]').click();
    const input = host.querySelector('.fupload-folder input[aria-label="New file name"]'); input.value = "notes.md";
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(host.querySelector('[data-path="ignored/notes.md"]')?.getAttribute("aria-current")).toBe("true"));
    expect(rpc).toHaveBeenCalledWith("fs.createFile", { ...source.scope, parent: "ignored", name: "notes.md" });
    expect(rpc.mock.calls.some(([method, params]) => method === "fs.read" && params.path === "ignored/notes.md")).toBe(true);
  });
}
const createFileFromMenu = (host, name) => {
  host.querySelector('[data-path="ignored"] .fupload-new .caret').click();
  host.querySelector('[data-path="ignored"] .fupload-new [data-action="file"]').click();
  const input = host.querySelector('.fupload-folder input[aria-label="New file name"]'); input.value = name;
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
};
const reads = (rpc, path) => rpc.mock.calls.filter(([method, params]) => method === "fs.read" && params.path === path).length;
// An ignored path sends no files push, so a body held from before a delete
// would otherwise come back as the new empty file's.
for (const [label, openOther] of [["shown", false], ["open behind another tab", true]]) {
  it(`shows a recreated file empty, not the body held from before it was deleted, when its tab is ${label}`, async () => {
    await rememberFileUploadSupport(device, { fs: { uploads: true, createDirectory: true, createFile: true } });
    const rpc = fakeRpc();
    rpc.listings.ignored.push({ name: "notes.txt", kind: "file", size: 11 }, { name: "other.txt", kind: "file", size: 0 });
    rpc.bodies["ignored/notes.txt"] = "old content";
    const { host } = mount(sources[2], rpc);
    await vi.waitFor(() => expect(host.querySelector('[data-path="ignored"] .fupload-new .caret')).toBeTruthy());
    host.querySelector('[data-path="ignored"]').click();
    await vi.waitFor(() => expect(host.querySelector('[data-path="ignored/notes.txt"]')).toBeTruthy());
    host.querySelector('[data-path="ignored/notes.txt"]').click();
    await vi.waitFor(() => expect(host.querySelector(".fpbody")?.textContent).toContain("old content"));
    if (openOther) {
      host.querySelector('[data-path="ignored/other.txt"]').click();
      await vi.waitFor(() => expect(host.querySelector(".fppath")?.textContent).toBe("ignored/other.txt"));
    }
    rpc.listings.ignored.splice(rpc.listings.ignored.findIndex((entry) => entry.name === "notes.txt"), 1);
    delete rpc.bodies["ignored/notes.txt"];
    const before = reads(rpc, "ignored/notes.txt");
    createFileFromMenu(host, "notes.txt");
    await vi.waitFor(() => expect(reads(rpc, "ignored/notes.txt")).toBeGreaterThan(before));
    await vi.waitFor(() => expect(host.querySelector(".fppath")?.textContent).toBe("ignored/notes.txt"));
    await vi.waitFor(() => expect(host.querySelector(".fpbody")?.textContent).not.toContain("old content"));
  });
}
const readBack = (body) => ({ path: "ignored/notes.txt", size: body.length, truncated: false, mime: "text/plain", content_b64: btoa(body), editable: true, encoding: "utf-8", revision: body ? "newer" : "e3b0c442" });
// The read made for a created file can answer after a newer body reached the
// cache (a save, a push): its answer, or its failure, must not undo that body.
for (const [label, settle] of [["answers", (held) => held.resolve(readBack(""))], ["fails", (held) => held.reject(new Error("gone"))]]) {
  it(`keeps a newer body cached while a created file's read was out, when that read ${label}`, async () => {
    await rememberFileUploadSupport(device, { fs: { uploads: true, createDirectory: true, createFile: true } });
    const rpc = fakeRpc();
    rpc.listings.ignored.push({ name: "notes.txt", kind: "file", size: 11 }, { name: "other.txt", kind: "file", size: 0 });
    rpc.bodies["ignored/notes.txt"] = "old content";
    const { host } = mount(sources[2], rpc);
    await vi.waitFor(() => expect(host.querySelector('[data-path="ignored"] .fupload-new .caret')).toBeTruthy());
    host.querySelector('[data-path="ignored"]').click();
    await vi.waitFor(() => expect(host.querySelector('[data-path="ignored/notes.txt"]')).toBeTruthy());
    host.querySelector('[data-path="ignored/notes.txt"]').click();
    await vi.waitFor(() => expect(host.querySelector(".fpbody")?.textContent).toContain("old content"));
    // Shown elsewhere, the created file's tab coming forward marks its read settled.
    host.querySelector('[data-path="ignored/other.txt"]').click();
    await vi.waitFor(() => expect(host.querySelector(".fppath")?.textContent).toBe("ignored/other.txt"));
    rpc.listings.ignored.splice(rpc.listings.ignored.findIndex((entry) => entry.name === "notes.txt"), 1);
    const held = {};
    rpc.heldReads["ignored/notes.txt"] = () => new Promise((resolve, reject) => Object.assign(held, { resolve, reject }));
    createFileFromMenu(host, "notes.txt");
    await vi.waitFor(() => expect(held.resolve).toBeTypeOf("function"));
    const address = { deviceId: device, entityId: "r", kind: "file", sub: "ignored/notes.txt" };
    await cacheFileBody({ deviceId: device, entityId: "r", path: "ignored/notes.txt", file: readBack("newer body") });
    expect(atob((await readCached(address)).value.file.content_b64)).toBe("newer body");
    settle(held);
    await vi.waitFor(() => expect(host.querySelector(".fppath")?.textContent).toBe("ignored/notes.txt"));
    expect(atob((await readCached(address))?.value.file.content_b64 || "")).toBe("newer body");
    await vi.waitFor(() => expect(host.querySelector(".fpbody")?.textContent).toContain("newer body"));
  });
}
it("keeps an upload and completion refresh through a Files remount", async () => {
  const rpc = fakeRpc();
  let finish;
  const callRpc = (method, params) => method === "fs.uploadFinish" ? new Promise((resolve) => { finish = async () => resolve(await rpc(method, params)); }) : rpc(method, params);
  const first = mount(sources[0], callRpc);
  const engine = uploadsFor(device); engines.push(engine);
  // The press is on the ignored folder's row, which the listing draws after
  // the root's own upload action: wait for that row's action.
  await untilDom(() => first.host.querySelector('[data-path="ignored"] [data-upload-action="upload"]'), first.host);
  chooseFiles(first.host);
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  first.view.dispose(); first.host.remove();
  const second = mount(sources[0], callRpc);
  await vi.waitFor(() => expect(second.host.querySelector(".fupload-status").textContent).toContain("Uploading"));
  await finish();
  await vi.waitFor(() => expect(second.host.querySelector('[data-path="ignored/new.txt"]')).toBeTruthy());
});
it("invalidates an ignored parent when an upload finishes while Files is unmounted", async () => {
  const rpc = fakeRpc();
  let finish;
  const callRpc = (method, params) => method === "fs.uploadFinish" ? new Promise((resolve) => { finish = async () => resolve(await rpc(method, params)); }) : rpc(method, params);
  const first = mount(sources[2], callRpc);
  const engine = uploadsFor(device); engines.push(engine);
  const address = { deviceId: device, entityId: "r", kind: "tree", sub: "ignored" };
  await vi.waitFor(() => expect(first.host.querySelector('[data-path="ignored"] [data-upload-action="upload"]')).toBeTruthy());
  first.host.querySelector('[data-path="ignored"]').click();
  await vi.waitFor(async () => expect((await readCached(address))?.value.entries).toEqual([]));
  chooseFiles(first.host);
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  first.view.dispose(); first.host.remove();
  await finish();
  await vi.waitFor(() => expect(engine.snapshot().recent[0]?.status).toBe("finished"));
  expect(await readCached(address)).toBeUndefined();
});
