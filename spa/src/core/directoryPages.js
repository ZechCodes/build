// Only a current capable greeting authorizes the paged request shape. A saved
// cursor from a newer bridge never rides a legacy fs.tree request.
import { bridgeCapabilities } from "./changeEvents.js";
import { cachedAddresses, mergeCachedRecordsTogether } from "./localCache.js";
import { visibleSyncReads } from "./syncVisibleScopes.js";
import { parentPath } from "./fileTreeModel.js";

export const chooseFilePushKind = (deviceId) =>
  bridgeCapabilities(deviceId).changes?.filesHints === true ? "files.hints" : "files";

const confinedHintPath = (path) => typeof path === "string" && path.length > 0 && path.length <= 4096
  && !path.startsWith("/") && !path.includes("\\")
  && path.split("/").every((part) => part && part !== "." && part !== "..");

export function normalizedDirectoryHint(files) {
  const paths = Array.isArray(files?.paths) ? files.paths : [];
  const invalid = !Array.isArray(files?.paths) || paths.length === 0 || paths.some((path) => !confinedHintPath(path));
  return { paths: paths.filter(confinedHintPath).slice(0, 200), truncated: Boolean(files?.truncated || invalid || paths.length > 200) };
}

export const directoryTouchedByHint = (dir, files) => files.truncated || files.paths.some((path) =>
  parentPath(path) === dir || path === dir || dir.startsWith(`${path}/`));

export const fileTouchedByHint = (path, files) => files.truncated || files.paths.some((changed) =>
  path === changed || path.startsWith(`${changed}/`));

const readsHint = (key, entityId, files) => key.entityId === entityId
  && (key.kind === "tree" || key.kind === "file" && fileTouchedByHint(key.sub, files));

/** Invalidations retain every held row. Only current visible readers ask for
 * first pages; a closed directory is refreshed when the reader opens it. */
export async function applyDirectoryHint({ deviceId, entityId, files, active = () => true }) {
  if (!active()) return false;
  const hint = normalizedDirectoryHint(files);
  const addresses = (await cachedAddresses({ deviceId, entityId, kind: "tree" }))
    .filter(({ sub }) => directoryTouchedByHint(sub, hint));
  if (!active()) return false;
  await mergeCachedRecordsTogether(addresses, (records) => records.map((record) =>
    active() && record?.value && !record.value.stale ? { ...record.value, stale: true } : null));
  if (!active()) return false;
  await Promise.all(visibleSyncReads(deviceId).filter(({ key }) => readsHint(key, entityId, hint))
    .map(({ read }) => read(active, { files: hint })));
  return true;
}

export const directoryPageParams = (deviceId, continuation = {}) =>
  bridgeCapabilities(deviceId).fs?.treePaged === true
    ? { paged: true, limit: 100, ...(continuation.cursor ? { cursor: continuation.cursor } : {}) }
    : {};

export const directoryListing = (answer, path) => ({
  path: answer?.path || path, entries: answer?.entries || [],
  ...(typeof answer?.has_more === "boolean" ? { has_more: answer.has_more, next_cursor: answer.next_cursor || null } : {}),
});

export function appendDirectoryPage(held, arriving, cursor) {
  if (!cursor || typeof arriving.has_more !== "boolean") return arriving;
  const entries = new Map((held?.entries || []).map((entry) => [entry.name, entry]));
  for (const entry of arriving.entries) entries.set(entry.name, entry);
  return { ...arriving, entries: [...entries.values()] };
}
