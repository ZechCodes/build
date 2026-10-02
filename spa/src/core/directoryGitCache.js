// Workspace sources and project directories have no board entity of their
// own. A board push is their invalidation, including while Changes is unmounted.
import { cachedEntityIds, mergeCachedTogether } from "./localCache.js";

const GIT_KINDS = ["refs", "status", "log", "unpushed", "diff"];
const isDirectory = (id) => id.startsWith("workspace:") || id.startsWith("project:");

/** Keep the last paintable values, but require a read next time they are used.
 *  The board names no source, so every cached directory on this device is
 *  conservatively stale. No pane or additional wire subscription is kept. */
export async function invalidateDirectoryGit(context) {
  const entities = await cachedEntityIds(context.deviceId);
  const addresses = entities.filter(isDirectory).flatMap((entityId) =>
    GIT_KINDS.map((kind) => ({ deviceId: context.deviceId, entityId, kind })));
  if (!addresses.length || !context.active()) return;
  await mergeCachedTogether(addresses, (values) => values.map((value) =>
    context.active() && value ? { ...value, stale: true } : null));
}
