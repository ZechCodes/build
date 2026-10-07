import { describe, it, expect, vi } from "vitest";
import { readUiRecord } from "../src/core/localUiStore.js";
import { createFileUploads } from "../src/core/fileUploads.js";
const tick = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };
const file = (name, text = "abcdef") => Object.assign(new Blob([text]), { name });
const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
const rpc = () => vi.fn(async (method, params) => {
  if (method === "fs.uploadBegin") return { upload_id: params.name, path: `${params.parent}/${params.name}`, chunk_bytes: 4 };
  if (method === "fs.uploadChunk") return { received: params.offset + atob(params.content_b64).length };
  if (method === "fs.uploadFinish") return { path: params.upload_id, size: 6 };
  return {};
});
describe("file upload engine", () => {
  it("flattens scope and streams bounded chunks with acknowledged progress", async () => {
    const callRpc = rpc(); const engine = createFileUploads({ callRpc });
    engine.enqueue({ scope: { workspace_id: "w", directory_id: "d" }, parent: "docs", files: [file("a")] });
    await vi.waitFor(() => expect(engine.snapshot().recent).toHaveLength(1));
    expect(callRpc.mock.calls[0]).toEqual(["fs.uploadBegin", { workspace_id: "w", directory_id: "d", parent: "docs", name: "a", size: 6 }]);
    expect(callRpc.mock.calls.filter(([m]) => m === "fs.uploadChunk").map(([, p]) => [p.offset, atob(p.content_b64)])).toEqual([[0, "abcd"], [4, "ef"]]);
    expect(engine.snapshot().recent[0]).toMatchObject({ status: "finished", received: 6 }); engine.dispose();
  });
  it("limits concurrency to two and cancels a late begin with abort", async () => {
    const begins = [deferred(), deferred()]; let index = 0;
    const callRpc = vi.fn((method) => method === "fs.uploadBegin" ? begins[index++].promise : Promise.resolve({}));
    const engine = createFileUploads({ callRpc });
    const ids = engine.enqueue({ scope: {}, parent: "", files: [file("a"), file("b"), file("c")] });
    await tick(); expect(index).toBe(2); engine.cancel(ids[0]); engine.cancel(ids[2]);
    begins[0].resolve({ upload_id: "late", path: "a", chunk_bytes: 4 }); await tick();
    expect(callRpc).toHaveBeenCalledWith("fs.uploadAbort", { upload_id: "late" });
    expect(engine.snapshot().recent.map((x) => x.status)).toEqual(["cancelled", "cancelled"]);
    engine.cancel(ids[1]); begins[1].resolve({ upload_id: "b", chunk_bytes: 4 }); await tick(); engine.dispose();
  });
  it("offers explicit replacement after collisions and prunes after thirty minutes", async () => {
    let time = 0; const callRpc = rpc(); callRpc.mockRejectedValueOnce(Object.assign(new Error("Exists"), { code: "already_exists" }));
    const engine = createFileUploads({ callRpc, now: () => time });
    const [id] = engine.enqueue({ scope: {}, parent: "", files: [file("a")] }); await tick();
    expect(engine.snapshot().recent[0]).toMatchObject({ errorCode: "already_exists", canRetry: true });
    engine.retry(id, { replace: true }); await vi.waitFor(() => expect(engine.snapshot().recent[0]?.status).toBe("finished"));
    expect(callRpc.mock.calls[1][1].replace).toBe(true); time = 1800001; expect(engine.snapshot().recent).toEqual([]); engine.prune(); expect(engine.snapshot().recent).toEqual([]); engine.dispose();
  });
});

describe("upload races and folder drops", () => {
  it("shows acknowledged progress and aborts after a chunk cancellation", async () => {
    const chunk = deferred(); const callRpc = rpc();
    callRpc.mockImplementationOnce(async () => ({ upload_id: "u", path: "a", chunk_bytes: 4 }));
    callRpc.mockImplementationOnce(() => chunk.promise);
    const engine = createFileUploads({ callRpc });
    const [id] = engine.enqueue({ scope: {}, parent: "", files: [file("a")] });
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledTimes(2));
    expect(engine.snapshot().active[0].received).toBe(0);
    engine.cancel(id); await tick();
    expect(callRpc).toHaveBeenLastCalledWith("fs.uploadAbort", { upload_id: "u" });
    chunk.resolve({ received: 4 }); await tick();
    expect(engine.snapshot().recent[0]).toMatchObject({ status: "cancelled", received: 4 });
    expect(callRpc).toHaveBeenLastCalledWith("fs.uploadAbort", { upload_id: "u" });
    expect(callRpc.mock.calls.some(([m]) => m === "fs.uploadFinish")).toBe(false); engine.dispose();
  });
  it("reports success when atomic finish wins cancellation", async () => {
    const finishing = deferred(); const base = rpc();
    const callRpc = vi.fn((method, params) => method === "fs.uploadFinish" ? finishing.promise : base(method, params));
    const engine = createFileUploads({ callRpc });
    const [id] = engine.enqueue({ scope: {}, files: [file("a")] });
    await vi.waitFor(() => expect(callRpc.mock.calls.some(([m]) => m === "fs.uploadFinish")).toBe(true));
    engine.cancel(id); finishing.resolve({ path: "a", size: 6 }); await tick();
    expect(engine.snapshot().recent[0].status).toBe("finished"); engine.dispose();
  });
  it("aborts a failed session and does not send a corrupt next offset", async () => {
    const callRpc = rpc();
    callRpc.mockImplementationOnce(async () => ({ upload_id: "u", chunk_bytes: 4 }));
    callRpc.mockResolvedValueOnce({ received: 3 });
    const engine = createFileUploads({ callRpc }); engine.enqueue({ scope: {}, files: [file("a")] });
    await vi.waitFor(() => expect(engine.snapshot().recent).toHaveLength(1));
    expect(engine.snapshot().recent[0].status).toBe("failed");
    expect(callRpc).toHaveBeenLastCalledWith("fs.uploadAbort", { upload_id: "u" }); engine.dispose();
  });
  it("walks all directory batches and creates each parent before beginning files", async () => {
    const leaf = (name) => ({ name, isFile: true, file: (done) => done(file(name)) });
    const folder = (name, children) => ({ name, isDirectory: true, createReader: () => {
      const batches = children.map((child) => [child]); batches.push([]);
      return { readEntries: (done) => done(batches.shift()) };
    } });
    const callRpc = rpc(); const onDirectory = vi.fn();
    const engine = createFileUploads({ callRpc: () => { throw new Error("wrong caller"); } });
    const root = folder("folder", [leaf("a"), folder("nested", [leaf("b")])]);
    await engine.enqueueDrop({ scope: { project_id: "p", source_id: "s" }, parent: "docs", rootId: "s", callRpc, onDirectory,
      dataTransfer: { items: [{ webkitGetAsEntry: () => root }] } });
    await vi.waitFor(() => expect(engine.snapshot().recent).toHaveLength(2));
    expect(callRpc.mock.calls[0]).toEqual(["fs.createDirectory", { project_id: "p", source_id: "s", parent: "docs", name: "folder" }]);
    const nestedCreate = callRpc.mock.calls.findIndex(([m, p]) => m === "fs.createDirectory" && p.name === "nested");
    const nestedBegin = callRpc.mock.calls.findIndex(([m, p]) => m === "fs.uploadBegin" && p.name === "b");
    expect(nestedCreate).toBeLessThan(nestedBegin);
    expect(onDirectory).toHaveBeenLastCalledWith({ scope: { project_id: "p", source_id: "s" }, parent: "docs/folder", path: "docs/folder/nested", rootId: "s" }); engine.dispose();
  });
  it("persists metadata without File bytes and restores failures without retry", async () => {
    const stateAddress = { deviceId: "upload-test", entityId: "", kind: "ui-uploads", sub: "restore" };
    const callRpc = vi.fn(async () => { throw new Error("Disconnected"); });
    const engine = createFileUploads({ callRpc, stateAddress }); await engine.ready;
    const [id] = engine.enqueue({ scope: { directory_id: "d" }, files: [file("a")] });
    await vi.waitFor(async () => expect((await readUiRecord(stateAddress))?.value.recent).toHaveLength(1));
    const saved = (await readUiRecord(stateAddress)).value.recent[0];
    expect(saved.file).toBeUndefined(); expect(saved.callRpc).toBeUndefined(); expect(saved.canRetry).toBe(false);
    engine.dispose();
    const restored = createFileUploads({ callRpc, stateAddress }); await restored.ready;
    expect(restored.snapshot().recent[0]).toMatchObject({ name: "a", canRetry: false, status: "failed" });
    expect(restored.retry(id)).toBe(false); restored.dispose();
  });
  it("automatically prunes completed rows after thirty minutes", async () => {
    vi.useFakeTimers(); const engine = createFileUploads({ callRpc: rpc() });
    engine.enqueue({ scope: {}, files: [file("a", "")] }); await tick();
    expect(engine.snapshot().recent).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1800000); expect(engine.snapshot().recent).toHaveLength(0);
    engine.dispose(); vi.useRealTimers();
  });
});

describe("folder failure handling", () => {
  const entry = (name = "folder") => ({ name, isDirectory: true, createReader: () => {
    let first = true;
    return { readEntries(done) { done(first ? [{ name: "a", isFile: true, file: (cb) => cb(file("a")) }] : []); first = false; } };
  } });
  it("continues through existing directories, records other folder errors, and uploads mixed files", async () => {
    const base = rpc(); const callRpc = vi.fn((method, params) => {
      if (method === "fs.createDirectory") throw Object.assign(new Error("Folder exists"), { code: "already_exists" });
      return base(method, params);
    });
    const engine = createFileUploads({ callRpc });
    await engine.enqueueDrop({ scope: {}, parent: "docs", destination: "Workspace/docs", dataTransfer: { items: [
      { webkitGetAsEntry: () => entry() }, { webkitGetAsEntry: () => null, getAsFile: () => file("loose") },
    ] } });
    await vi.waitFor(() => expect(engine.snapshot().recent).toHaveLength(2));
    expect(engine.snapshot().recent.find((item) => item.name === "a").destination).toBe("Workspace/docs/folder");
    callRpc.mockImplementationOnce(() => { throw new Error("Permission denied"); });
    await engine.enqueueDrop({ scope: {}, dataTransfer: { items: [{ webkitGetAsEntry: () => entry("denied") }] } });
    expect(engine.snapshot().recent.find((item) => item.name === "denied")).toMatchObject({ status: "failed", canRetry: false, error: "Permission denied" });
    engine.dispose();
  });
  it("still records the original failure if abort itself throws", async () => {
    const callRpc = vi.fn((method) => {
      if (method === "fs.uploadBegin") return Promise.resolve({ upload_id: "u", chunk_bytes: 4 });
      throw new Error(method === "fs.uploadAbort" ? "Abort disconnected" : "Chunk disconnected");
    });
    const engine = createFileUploads({ callRpc }); engine.enqueue({ scope: {}, files: [file("a")] });
    await vi.waitFor(() => expect(engine.snapshot().recent).toHaveLength(1));
    expect(engine.snapshot().recent[0].error).toBe("Chunk disconnected"); engine.dispose();
  });
});
