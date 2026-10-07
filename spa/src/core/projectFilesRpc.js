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

export function projectFilesRpc(context, primarySourceId, sourceOwner = null) {
  return async (method, params, ...options) => {
    const request = await whenGreeted(context, () => {
      if (uploadSessionMethods.has(method)) return context.rpc(method, params, ...options);
      if (sourceOwner && sourceGeneration(sourceOwner.project, params.source_id) !== sourceGeneration(sourceOwner.currentProject(), params.source_id)) {
        throw new Error("This project folder moved. Reopen Files before reading or saving it.");
      }
      if (!params.source_id || bridgeCapabilities(context.deviceId).fs?.projectSources) return context.rpc(method, params, ...options);
      if (params.source_id !== primarySourceId) throw new Error("Update the bridge to browse this project folder.");
      const { source_id: _source, ...legacy } = params;
      return context.rpc(method, legacy, ...options);
    });
    if (!request) throw new Error("This machine is unavailable.");
    return request.sent;
  };
}
