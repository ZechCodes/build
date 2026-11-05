// Only a current capable greeting authorizes the paged request shape. A saved
// cursor from a newer bridge never rides a legacy fs.tree request.
import { bridgeCapabilities, onBridgeGreeted } from "./changeEvents.js";
import { cachedAddresses, mergeCachedRecordsTogether, readCached, recordWriteOf } from "./localCache.js";
import { visibleSyncReads, visibleTreePaths } from "./syncVisibleScopes.js";
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

const visibleRoot = (deviceId, entityId) => visibleTreePaths(deviceId, entityId).includes("");

async function legacyRootForVisibleReader(deviceId, entityId, files) {
  const root = files?.root;
  if (chooseFilePushKind(deviceId) !== "files" || root?.path !== "" || !Array.isArray(root.entries)) return null;
  if (!visibleRoot(deviceId, entityId)) return null;
  const address = { deviceId, entityId, kind: "tree", sub: "" };
  const written = recordWriteOf(await readCached(address));
  return { address, written, value: { path: "", entries: root.entries } };
}

const staleListing = (record) => record?.value && !record.value.stale ? { ...record.value, stale: true } : null;

async function cacheDirectoryHint(deviceId, entityId, files, hint, active) {
  const root = await legacyRootForVisibleReader(deviceId, entityId, files);
  const held = (await cachedAddresses({ deviceId, entityId, kind: "tree" }))
    .filter(({ sub }) => directoryTouchedByHint(sub, hint));
  const addresses = root && !held.some(({ sub }) => sub === "") ? [...held, root.address] : held;
  let acceptedRoot = false;
  const changed = await mergeCachedRecordsTogether(addresses, (records) => records.map((record, index) => {
    if (!active()) return null;
    if (root && addresses[index].sub === "") {
      if (recordWriteOf(record) !== root.written) return null;
      if (chooseFilePushKind(deviceId) === "files" && visibleRoot(deviceId, entityId)) {
        acceptedRoot = true;
        return root.value;
      }
    }
    return staleListing(record);
  }));
  return changed && acceptedRoot ? [""] : [];
}

async function applyCurrentDirectoryHint(deviceId, entityId, files, active) {
  const hint = normalizedDirectoryHint(files);
  const fulfilledDirectories = await cacheDirectoryHint(deviceId, entityId, files, hint, active);
  if (!active()) return false;
  const options = { files: hint, ...(fulfilledDirectories.length ? { fulfilledDirectories } : {}) };
  await Promise.all(visibleSyncReads(deviceId).filter(({ key }) => readsHint(key, entityId, hint))
    .map(({ read }) => read(active, options)));
  return true;
}

/** Hints retain held rows. A visible legacy root may reuse the complete body
 * already carried by its frame; only other visible invalidations need reads. */
export async function applyDirectoryHint({ deviceId, entityId, files, active = () => true }) {
  if (!active()) return false;
  let current = true;
  const stop = onBridgeGreeted((changed) => { if (changed === deviceId) current = false; });
  try { return await applyCurrentDirectoryHint(deviceId, entityId, files, () => current && active()); }
  finally { stop(); }
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
