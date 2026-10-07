// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
vi.mock("../src/core/fileUploadRpc.js", () => ({ fileUploadRpc: (_context, callRpc) => callRpc }));
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { scopeFor } = await import("../src/core/cacheScope.js");
const { readCached, wipeCache } = await import("../src/core/localCache.js");
const { wipeUiRecords } = await import("../src/core/localUiStore.js");
const { rememberFileUploadSupport } = await import("../src/core/fileUploadSupport.js");
const { uploadsFor } = await import("../src/core/fileUploads.js");
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
  let file;
  const callRpc = vi.fn(async (method, params) => {
    if (method === "fs.tree") return { path: params.path, entries: [...(listings[params.path] || [])] };
    if (method === "fs.createDirectory") { listings[params.parent].push({ name: params.name, kind: "dir" }); return { path: `${params.parent}/${params.name}` }; }
    if (method === "fs.uploadBegin") { file = params; return { upload_id: "u", path: `${params.parent}/${params.name}`, chunk_bytes: 2 }; }
    if (method === "fs.uploadChunk") return { received: params.offset + atob(params.content_b64).length };
    if (method === "fs.uploadFinish") { listings[file.parent].push({ name: file.name, kind: "file", size: file.size }); return { path: `${file.parent}/${file.name}`, size: file.size }; }
    throw new Error(`Unexpected ${method}`);
  });
  return callRpc;
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
it("keeps an upload and completion refresh through a Files remount", async () => {
  const rpc = fakeRpc();
  let finish;
  const callRpc = (method, params) => method === "fs.uploadFinish" ? new Promise((resolve) => { finish = async () => resolve(await rpc(method, params)); }) : rpc(method, params);
  const first = mount(sources[0], callRpc);
  const engine = uploadsFor(device); engines.push(engine);
  await vi.waitFor(() => expect(first.host.querySelector('[data-upload-action="upload"]')).toBeTruthy());
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
