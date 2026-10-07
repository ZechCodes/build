export const UPLOAD_RETENTION_MS = 30 * 60 * 1000;
export const joinUploadPath = (parent, name) => parent ? `${parent.replace(/\/$/, "")}/${name}` : name;
export const uploadErrorCode = (error) => error?.code || error?.error?.code || null;
export const uploadMetadata = (item, canRetry = false) => {
  const { file: _file, callRpc: _callRpc, uploadId: _uploadId, cancelled: _cancelled, replace: _replace, aborting: _aborting, ...metadata } = item;
  return { ...metadata, scope: { ...metadata.scope }, canRetry };
};
export function encodeUploadBytes(buffer) {
  const bytes = new Uint8Array(buffer);
  const parts = [];
  for (let offset = 0; offset < bytes.length; offset += 32768) {
    parts.push(String.fromCharCode(...bytes.subarray(offset, offset + 32768)));
  }
  return btoa(parts.join(""));
}
const entryFile = (entry) => new Promise((resolve, reject) => entry.file(resolve, reject));
const entryBatch = (reader) => new Promise((resolve, reject) => reader.readEntries(resolve, reject));
export async function directoryEntries(entry) {
  const reader = entry.createReader();
  const entries = [];
  let batch = await entryBatch(reader);
  while (batch.length) {
    entries.push(...batch);
    batch = await entryBatch(reader);
  }
  return entries;
}
export async function walkUploadEntry(entry, parent, { directory, enqueue }) {
  if (entry.isFile) return enqueue(parent, await entryFile(entry));
  if (!entry.isDirectory) return;
  const path = await directory(parent, entry.name);
  for (const child of await directoryEntries(entry)) {
    await walkUploadEntry(child, path, { directory, enqueue });
  }
}
