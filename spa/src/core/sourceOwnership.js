// Rendering uses cached project/support records only. Dispatch separately
// requires the current accepted greeting, with an epoch fence when available.
import { captureDeviceGreeting, contextFor, whenGreeted } from "./deviceContexts.js";
import { predatesEnvironments, readEnvironmentSupportRecord } from "./environmentSupport.js";

export const SOURCE_READ_ONLY = "Source ownership is unknown or belongs to the environment. This folder is read-only here. Refresh the project before changing it.";

export async function readSourceOwnershipEvidence(deviceId) {
  return { support: await readEnvironmentSupportRecord(deviceId) };
}

export function sourceOwnershipState(project, evidence) {
  if (Array.isArray(project?.added_sources)) return "known";
  if (project?.environment_id || project?.environment_name) return "unknown";
  return predatesEnvironments(evidence?.support) ? "legacy" : "unknown";
}

/** Cached eligibility for controls, not permission to dispatch an RPC. */
export function projectSourceWritable(project, sourceId, evidence) {
  const ownership = sourceOwnershipState(project, evidence);
  return ownership === "legacy" || ownership === "known" && project.added_sources.some(source => source.id === sourceId);
}

function sourceDispatchAllowed(project, sourceId, evidence, authority) {
  if (!authority?.current()) return false;
  if (!authority.epoch) {
    const support = authority.capabilities?.environments;
    return predatesEnvironments(support) && projectSourceWritable(project, sourceId, { support });
  }
  if (sourceOwnershipState(project, evidence) === "legacy" && evidence.support.bridge_epoch !== authority.epoch) return false;
  return projectSourceWritable(project, sourceId, evidence);
}

/** Re-read after the existing greeting wait, immediately before dispatch. A
 * retained control or queued action cannot carry old legacy evidence forward. */
export async function withWritableProjectSource(deviceId, context, readProject, sourceId, send) {
  const request = await whenGreeted(context, async (stands = () => true) => {
    const evidence = await readSourceOwnershipEvidence(deviceId);
    const project = await readProject();
    const id = sourceId || project?.sources?.[0]?.id;
    const authority = captureDeviceGreeting(context, { allowLegacy: true });
    if (!stands() || !project || !sourceDispatchAllowed(project, id, evidence, authority)) throw new Error(SOURCE_READ_ONLY);
    return send();
  });
  if (!request) throw new Error(SOURCE_READ_ONLY);
  return request.sent;
}

const sourceWrites = new Set(["project.update_source", "project.remove_source", "project.sync_source", "project.set_remote"]);

export function guardProjectSourceRpc(deviceId, callRpc, readProject) {
  return (method, params, ...options) => sourceWrites.has(method)
    ? withWritableProjectSource(deviceId, contextFor(deviceId), readProject, params?.source_id, () => callRpc(method, params, ...options))
    : callRpc(method, params, ...options);
}
