// Ownership comes from cached project/support records. The existing accepted
// greeting epoch fences legacy evidence; it is not a connection-status view.
import { captureDeviceGreeting, contextFor, whenGreeted } from "./deviceContexts.js";
import { predatesEnvironments, readEnvironmentSupportRecord } from "./environmentSupport.js";

export const SOURCE_READ_ONLY = "Source ownership is unknown or belongs to the environment. This folder is read-only here. Refresh the project before changing it.";

export async function readSourceOwnershipEvidence(deviceId, context = contextFor(deviceId)) {
  const support = await readEnvironmentSupportRecord(deviceId);
  return { support, authority: captureDeviceGreeting(context) };
}

export function sourceOwnershipState(project, evidence) {
  if (Array.isArray(project?.added_sources)) return "known";
  if (project?.environment_id || project?.environment_name) return "unknown";
  return currentLegacyEvidence(evidence) ? "legacy" : "unknown";
}

function currentLegacyEvidence(evidence) {
  const { support, authority } = evidence || {};
  return predatesEnvironments(support) && authority?.current()
    && typeof support.bridge_epoch === "string" && support.bridge_epoch === authority.epoch;
}

export function projectSourceWritable(project, sourceId, evidence) {
  const ownership = sourceOwnershipState(project, evidence);
  return ownership === "legacy" || ownership === "known" && project.added_sources.some(source => source.id === sourceId);
}

/** Re-read after the existing greeting wait, immediately before dispatch. A
 * retained control or queued action cannot carry old legacy evidence forward. */
export async function withWritableProjectSource(deviceId, context, readProject, sourceId, send) {
  const request = await whenGreeted(context, async (stands = () => true) => {
    const evidence = await readSourceOwnershipEvidence(deviceId, context);
    const project = await readProject();
    const id = sourceId || project?.sources?.[0]?.id;
    if (!stands() || !project || !projectSourceWritable(project, id, evidence)) throw new Error(SOURCE_READ_ONLY);
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
