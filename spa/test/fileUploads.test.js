import { describe, it, expect, vi } from "vitest";
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
    expect(callRpc.mock.calls[1][1].replace).toBe(true); time = 1800001; engine.prune(); expect(engine.snapshot().recent).toEqual([]); engine.dispose();
  });
});
