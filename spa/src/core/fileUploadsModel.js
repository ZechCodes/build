export const MAX_UPLOAD_BYTES = 256 * 1024 * 1024;
export const uploadNameValid = (name) => typeof name === "string" && name.length > 0
  && name !== "." && name !== ".." && !/[\/\\\0]/.test(name);
export const uploadNameError = () => Object.assign(new Error("Choose a file or folder name without slashes, backslashes, or NUL characters. Names cannot be empty, '.' or '..'."), { code: "invalid_name" });
export function uploadInputError(name, size) {
  if (!uploadNameValid(name)) return uploadNameError();
  if (size > MAX_UPLOAD_BYTES) return Object.assign(new Error("This file exceeds the 256 MiB upload limit. Choose a smaller file."), { code: "file_too_large" });
  return null;
}
export const UPLOAD_RETENTION_MS = 30 * 60 * 1000;
export const joinUploadPath = (parent, name) => parent ? `${parent.replace(/\/$/, "")}/${name}` : name;
export const uploadErrorCode = (error) => error?.code || error?.error?.code || null;
export const uploadMetadata = (item, canRetry = false) => {
  const { file: _file, callRpc: _callRpc, onFinished: _onFinished, uploadId: _uploadId, cancelled: _cancelled, replace: _replace, aborting: _aborting, ...metadata } = item;
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
  if (!uploadNameValid(entry.name)) throw uploadNameError();
  if (entry.isFile) return enqueue(parent, await entryFile(entry));
  if (!entry.isDirectory) return;
  const path = await directory(parent, entry.name);
  for (const child of await directoryEntries(entry)) {
    await walkUploadEntry(child, path, { directory, enqueue });
  }
}
