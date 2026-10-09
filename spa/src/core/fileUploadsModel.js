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

/** The inline drafts a directory row's New action opens, in menu order. */
export const NEW_ENTRIES = Object.freeze({
  file: Object.freeze({ label: "New file", field: "New file name", placeholder: "File name", method: "fs.createFile", capability: "createFile", invalid: "Enter a file name without slashes.", failed: "Could not create file." }),
  folder: Object.freeze({ label: "New folder", field: "New folder name", placeholder: "Folder name", method: "fs.createDirectory", capability: "createDirectory", invalid: "Enter a folder name without slashes.", failed: "Could not create folder." }),
});

/** What the New action offers this bridge: a kind per announced verb. */
export const newEntryKinds = (support) => Object.keys(NEW_ENTRIES).filter((kind) => support?.[NEW_ENTRIES[kind].capability] === true);

/** The line under a refused draft. A file name already taken says so plainly;
 *  a folder keeps the bridge's own sentence, as it always has. */
export const newEntryError = (kind, error) =>
  kind === "file" && uploadErrorCode(error) === "already_exists" ? "Already exists" : error?.message || NEW_ENTRIES[kind].failed;
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
