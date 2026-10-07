import { afterAll, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
let capabilities = { fs: {} };
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: () => capabilities }));
vi.mock("../src/core/deviceContexts.js", () => ({ whenGreeted: (context, dispatch) => context.whenGreeted(dispatch) }));
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { capabilitiesOf } = await import("../src/core/bridgeApi/v1/index.js");
const { fileUploadRpc } = await import("../src/core/fileUploadRpc.js");
const { readFileUploadSupport, rememberFileUploadSupport } = await import("../src/core/fileUploadSupport.js");
const { wipeCache } = await import("../src/core/localCache.js");
beforeEach(async () => { capabilities = { fs: {} }; await wipeCache(); });
const context = () => ({ deviceId: "d", rpc: vi.fn(async () => ({})), whenGreeted: async (dispatch) => ({ sent: dispatch() }) });
it("derives flags independently from the announced verbs", () => {
  expect(capabilitiesOf({ api_version: "3.13.0", capabilities: ["fs.uploadBegin"] }).fs).toMatchObject({ uploads: true, createDirectory: false });
  expect(capabilitiesOf({ api_version: "3.13.0", capabilities: ["fs.createDirectory"] }).fs).toMatchObject({ uploads: false, createDirectory: true });
});
it("remembers support for cold mounts and older greetings", async () => {
  expect(await readFileUploadSupport("d")).toEqual({ uploads: false, createDirectory: false });
  await rememberFileUploadSupport("d", { fs: { uploads: true, createDirectory: true } });
  expect(await readFileUploadSupport("d")).toEqual({ uploads: true, createDirectory: true });
  await rememberFileUploadSupport("d", { fs: {} });
  expect(await readFileUploadSupport("d")).toEqual({ uploads: false, createDirectory: false });
});
it("refuses older bridges before beginning writes", async () => {
  const machine = context();
  for (const method of ["fs.uploadBegin", "fs.createDirectory"]) await expect(fileUploadRpc(machine)(method, { parent: "", name: "file" })).rejects.toThrow("Update the bridge");
  expect(machine.rpc).not.toHaveBeenCalled();
});
it("waits for greeting and preserves selectors and options", async () => {
  const machine = context();
  let release;
  machine.whenGreeted = async (dispatch) => { await new Promise((resolve) => { release = resolve; }); return { sent: dispatch() }; };
  const params = { project_id: "p", source_id: "s", parent: "docs", name: "file", size: 4, replace: true };
  const sending = fileUploadRpc(machine)("fs.uploadBegin", params, { priority: "background" });
  expect(machine.rpc).not.toHaveBeenCalled();
  capabilities.fs.uploads = true;
  release();
  await sending;
  expect(machine.rpc).toHaveBeenCalledWith("fs.uploadBegin", params, { priority: "background" });
});
it("allows cleanup and chunk sends after capabilities change", async () => {
  const machine = context();
  for (const method of ["fs.uploadChunk", "fs.uploadFinish", "fs.uploadAbort"]) await fileUploadRpc(machine)(method, { upload_id: "u" });
  expect(machine.rpc).toHaveBeenCalledTimes(3);
});

afterAll(() => { vi.doUnmock("../src/core/changeEvents.js"); vi.doUnmock("../src/core/deviceContexts.js"); vi.resetModules(); });
