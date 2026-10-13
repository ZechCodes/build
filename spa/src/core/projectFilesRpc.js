import { whenGreeted } from "./deviceContexts.js";
import { bridgeCapabilities } from "./changeEvents.js";
const uploadSessionMethods = new Set(["fs.uploadChunk", "fs.uploadFinish", "fs.uploadAbort"]);

/** Older bridges can read only the primary source. Never send their unknown
 * source field: an old serde decoder could ignore it and read the wrong root. */
const sourceGeneration = (project, sourceId) => {
  if (!sourceId) return project?.path || null;
  const source = project?.sources?.find((entry) => entry.id === sourceId);
  return source ? JSON.stringify([source.id, source.path]) : null;
};

const sourcePath = (project, sourceId) => sourceId
  ? project?.sources?.find((entry) => entry.id === sourceId)?.path
  : project?.path;

function sourceStillCurrent(sourceOwner, params) {
  const current = sourceOwner.currentProject();
  if (Object.hasOwn(params, "source_path")) return params.source_path === sourcePath(current, params.source_id);
  return sourceGeneration(sourceOwner.project, params.source_id) === sourceGeneration(current, params.source_id);
}

function refuseMovedSource(sourceOwner, params) {
  if (sourceOwner && !sourceStillCurrent(sourceOwner, params)) {
    throw new Error("This project folder moved. Reopen Files before reading or saving it.");
  }
}

/** The folder this Files face opened the source at, for a bridge that
 * announces it: the bridge refuses the call itself once the source moved. */
function withSourcePath(params, sourceOwner, fs) {
  if (!fs?.projectSourcePath) {
    const { source_path: _path, ...supported } = params;
    return supported;
  }
  if (Object.hasOwn(params, "source_path") || !params.source_id || !sourceOwner) return params;
  const opened = sourcePath(sourceOwner.project, params.source_id);
  return opened ? { ...params, source_path: opened } : params;
}

// Root reconciliation keeps the explorer mounted. Its legacy selector must
// follow the cached project roster, including after a queued greeting.
const legacyPrimarySourceId = (sourceOwner, openedId) => sourceOwner
  ? sourceOwner.currentProject()?.sources?.[0]?.id
  : openedId;

function dispatch(context, primarySourceId, sourceOwner, method, params, options) {
  if (uploadSessionMethods.has(method)) return context.rpc(method, params, ...options);
  refuseMovedSource(sourceOwner, params);
  const fs = bridgeCapabilities(context.deviceId).fs;
  if (!params.source_id || fs?.projectSources) return context.rpc(method, withSourcePath(params, sourceOwner, fs), ...options);
  if (params.source_id !== legacyPrimarySourceId(sourceOwner, primarySourceId)) throw new Error("Update the bridge to browse this project folder.");
  const { source_id: _source, source_path: _path, ...legacy } = params;
  return context.rpc(method, legacy, ...options);
}

export function projectFilesRpc(context, primarySourceId, sourceOwner = null) {
  return async (method, params, ...options) => {
    const request = await whenGreeted(context, () => dispatch(context, primarySourceId, sourceOwner, method, params, options));
    if (!request) throw new Error("This machine is unavailable.");
    return request.sent;
  };
}
