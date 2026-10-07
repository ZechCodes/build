// Per-device upload ownership outlives the Files view. Only completed metadata
// reaches UI storage; browser File objects and upload sessions remain in memory.
import { App } from "../appState.js";
import { readUiRecord, writeUiRecord } from "./localUiStore.js";
import { UPLOAD_RETENTION_MS, uploadInputError, uploadNameValid, uploadNameError, joinUploadPath, uploadErrorCode, uploadMetadata, encodeUploadBytes, walkUploadEntry } from "./fileUploadsModel.js";

export { uploadNameValid } from "./fileUploadsModel.js";

const instances = new Map();
export function uploadsFor(deviceId, { callRpc } = {}) {
  if (!instances.has(deviceId)) {
    const accountEpoch = App.accountEpoch;
    let context;
    instances.set(deviceId, createFileUploads({
      callRpc: callRpc || (async (method, params) => {
        const { contextFor } = await import("./deviceContexts.js");
        if (App.accountEpoch !== accountEpoch) throw new Error("This account changed. Reopen Files before uploading.");
        context ||= contextFor(deviceId);
        return context.rpc(method, params);
      }),
      stateAddress: { deviceId, entityId: "", kind: "ui-uploads", sub: "" },
    }));
  }
  return instances.get(deviceId);
}
export function retireFileUploads(deviceId) {
  instances.get(deviceId)?.dispose();
  instances.delete(deviceId);
}
export function resetFileUploads() {
  for (const deviceId of [...instances.keys()]) retireFileUploads(deviceId);
}
const readUploadBlob = (blob) => {
  if (blob.arrayBuffer) return blob.arrayBuffer();
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(reader.error);
    reader.readAsArrayBuffer(blob);
  });
};
const frozenItem = (item) => Object.freeze({ ...uploadMetadata(item, !!item.file && !item.permanentlyInvalid && item.status !== "finished"), scope: Object.freeze({ ...item.scope }) });

export function createFileUploads({ callRpc, now = Date.now, stateAddress = null }) {
  const send = (item, method, params) => (item.callRpc || callRpc)(method, params);
  const items = new Map();
  const listeners = new Set();
  let running = 0;
  let disposed = false;
  let persistence = Promise.resolve();
  const snapshot = () => {
    const all = [...items.values()].map(frozenItem);
    return Object.freeze({
      active: Object.freeze(all.filter((item) => item.finishedAt === null)),
      recent: Object.freeze(all.filter((item) => item.finishedAt !== null && now() - item.finishedAt < UPLOAD_RETENTION_MS)),
    });
  };
  const announce = () => listeners.forEach((listener) => listener(snapshot()));
  const persist = () => {
    if (!stateAddress || disposed) return;
    const recent = [...items.values()].filter((item) => item.finishedAt !== null).map((item) => uploadMetadata(item));
    persistence = persistence.catch(() => {}).then(() => disposed ? undefined : writeUiRecord(stateAddress, { recent })).catch(() => {});
  };
  const prune = () => {
    for (const [id, item] of items) {
      if (item.finishedAt !== null && now() - item.finishedAt >= UPLOAD_RETENTION_MS) items.delete(id);
    }
    announce(); persist();
  };
  const finish = (item, status, error = null) => {
    Object.assign(item, { status, finishedAt: now(), error: error?.message || null, errorCode: uploadErrorCode(error) });
    announce(); persist();
  };
  const abort = async (item) => {
    if (!item.uploadId) return;
    if (!item.aborting) item.aborting = Promise.resolve().then(() => send(item, "fs.uploadAbort", { upload_id: item.uploadId })).catch(() => {});
    await item.aborting;
  };
  const sendChunks = async (item, chunkBytes) => {
    let offset = 0;
    while (offset < item.size && !item.cancelled) {
      const end = Math.min(item.size, offset + chunkBytes);
      const buffer = await readUploadBlob(item.file.slice(offset, end));
      if (item.cancelled) return;
      const result = await send(item, "fs.uploadChunk", { upload_id: item.uploadId, offset, content_b64: encodeUploadBytes(buffer) });
      if (!Number.isSafeInteger(result.received) || result.received !== end) throw new Error("The upload returned an unexpected byte count. Retry the file.");
      item.received = result.received;
      offset = end;
      announce();
    }
  };
  const perform = async (item) => {
    const params = { ...item.scope, parent: item.parent, name: item.name, size: item.size };
    if (item.replace) params.replace = true;
    const started = await send(item, "fs.uploadBegin", params);
    item.uploadId = started.upload_id;
    item.path = started.path;
    if (item.cancelled) return;
    if (!Number.isSafeInteger(started.chunk_bytes) || started.chunk_bytes <= 0) throw new Error("The upload returned an invalid chunk size.");
    await sendChunks(item, started.chunk_bytes);
    if (item.cancelled) return;
    const result = await send(item, "fs.uploadFinish", { upload_id: item.uploadId });
    // Finish is atomic. If it won a cancellation race, report the saved file.
    Object.assign(item, { path: result.path, size: result.size, received: result.size });
    if (!disposed) {
      try { await item.onFinished?.({ scope: item.scope, parent: item.parent, path: item.path, rootId: item.rootId }); }
      catch { /* The file is saved even if the cache cannot be refreshed yet. */ }
    }
    finish(item, "finished");
  };
  const run = async (item) => {
    item.status = "uploading"; running += 1; announce();
    try {
      await perform(item);
      if (item.finishedAt === null) { await abort(item); finish(item, "cancelled"); }
    } catch (error) {
      await abort(item);
      finish(item, item.cancelled ? "cancelled" : "failed", item.cancelled ? null : error);
    } finally {
      running -= 1; pump();
    }
  };
  const pump = () => {
    if (disposed) return;
    for (const item of items.values()) {
      if (running >= 2) return;
      if (item.status === "queued") void run(item);
    }
  };
  const add = ({ scope, parent = "", rootId = null, destination = "", callRpc: itemRpc, onFinished, file, name = file?.name, size = file?.size || 0 }) => {
    const id = globalThis.crypto.randomUUID();
    items.set(id, { id, scope: { ...scope }, parent, rootId, destination, callRpc: itemRpc, onFinished, file, name, size, received: 0,
      path: joinUploadPath(parent, name), status: "queued", finishedAt: null, error: null, errorCode: null,
      cancelled: false, uploadId: null });
    const error = uploadInputError(name, size);
    if (error) {
      const item = items.get(id);
      item.permanentlyInvalid = true;
      finish(item, "failed", error);
    }
    return id;
  };
  const enqueue = ({ files, ...target }) => {
    if (disposed) return [];
    const ids = Array.from(files).map((file) => add({ ...target, file }));
    announce(); pump(); return ids;
  };
  const cancel = (id) => {
    const item = items.get(id);
    if (!item || item.finishedAt !== null) return;
    item.cancelled = true;
    if (item.status === "queued") finish(item, "cancelled");
    else { item.status = "cancelling"; void abort(item); announce(); }
  };
  const retry = (id, { replace = false } = {}) => {
    const item = items.get(id);
    if (!item?.file || item.permanentlyInvalid || item.finishedAt === null || item.status === "finished") return false;
    Object.assign(item, { status: "queued", finishedAt: null, error: null, errorCode: null, received: 0,
      cancelled: false, uploadId: null, aborting: null, replace });
    announce(); persist(); pump(); return true;
  };
  const enqueueDrop = async ({ dataTransfer, onDirectory, ...target }) => {
    if (disposed) return [];
    const droppedItems = Array.from(dataTransfer.items || []);
    const entries = droppedItems.map((item) => item.webkitGetAsEntry?.()).filter(Boolean);
    if (!entries.length) return enqueue({ ...target, files: dataTransfer.files || [] });
    const looseFiles = droppedItems.filter((item) => !item.webkitGetAsEntry?.()).map((item) => item.getAsFile?.()).filter(Boolean);
    const ids = enqueue({ ...target, files: looseFiles });
    const directory = async (parent, name) => {
      if (disposed) throw new Error("The upload was stopped.");
      if (!uploadNameValid(name)) throw uploadNameError();
      try { await (target.callRpc || callRpc)("fs.createDirectory", { ...target.scope, parent, name }); }
      catch (error) { if (uploadErrorCode(error) !== "already_exists") throw error; }
      const path = joinUploadPath(parent, name);
      if (!disposed) onDirectory?.({ scope: target.scope, parent, path, rootId: target.rootId });
      return path;
    };
    const queueFile = (parent, file) => {
      const relative = parent.slice((target.parent || "").length).replace(/^\//, "");
      const destination = relative ? joinUploadPath(target.destination || target.parent || "", relative) : target.destination;
      ids.push(...enqueue({ ...target, parent, destination, files: [file] }));
    };
    for (const entry of entries) {
      try { await walkUploadEntry(entry, target.parent || "", { directory, enqueue: queueFile }); }
      catch (error) {
        if (disposed) break;
        const id = add({ ...target, name: entry.name });
        finish(items.get(id), "failed", error); ids.push(id);
      }
    }
    return ids;
  };
  const ready = (async () => {
    if (!stateAddress || disposed) return;
    const saved = await readUiRecord(stateAddress).catch(() => null);
    if (disposed) return;
    for (const item of saved?.value?.recent || []) {
      if (!items.has(item.id) && now() - item.finishedAt < UPLOAD_RETENTION_MS) items.set(item.id, { ...item, file: null });
    }
    announce();
  })();
  const timer = setInterval(prune, 60000);
  timer.unref?.();
  return { snapshot, enqueue, enqueueDrop, cancel, retry, ready, prune,
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    dispose() {
      disposed = true;
      for (const item of items.values()) {
        if (item.finishedAt === null) { item.cancelled = true; void abort(item); }
      }
      items.clear(); clearInterval(timer); listeners.clear();
    },
  };
}
