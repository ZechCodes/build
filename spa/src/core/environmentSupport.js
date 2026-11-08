// Rendering remembers the latest greeting; dispatch requires the live capability.
export { environmentWizardSupported } from "./writeOperationProgress.js";
import { mergeCachedAtomically, readCached } from "./localCache.js";

const FLAGS = ["list", "createProject", "create", "addedSources", "archive", "projectRename", "environmentRename", "secrets"];
const supportOf = (capabilities) => Object.fromEntries(FLAGS.map((flag) => [flag, capabilities?.[flag] === true]));

export const environmentSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "environment-support" });

// Missing flags are not negative evidence: only a complete cached greeting
// without any environment capability identifies a pre-environment device.
const establishedFlags = FLAGS.filter(flag => !["projectRename", "environmentRename"].includes(flag));
export const predatesEnvironments = support => establishedFlags.every(flag => support?.[flag] === false)
  && renameUnsupported(support);
// Pre-split all-false greetings retain legacy source ownership evidence.
// Their positive rename flag grants neither of the new rename actions.
const renameUnsupported = support => support?.projectRename === false && support?.environmentRename === false
  || support?.rename === false && support?.projectRename !== true && support?.environmentRename !== true;

export async function readEnvironmentSupportRecord(deviceId = "") {
  return (await readCached(environmentSupportAddress(deviceId)))?.value ?? null;
}

export function rememberEnvironmentSupport(deviceId, capabilities, current = () => true, bridgeEpoch = null) {
  if (!deviceId) return Promise.resolve(false);
  const support = { ...supportOf(capabilities?.environments), bridge_epoch: bridgeEpoch };
  // Every accepted greeting announces its evidence, including a new opening
  // in the same bridge epoch. No wall-clock freshness or separate epoch exists.
  return mergeCachedAtomically(environmentSupportAddress(deviceId), () => current() ? support : null);
}

export async function readEnvironmentSupport(deviceId) {
  const held = deviceId ? await readEnvironmentSupportRecord(deviceId) : null;
  return supportOf(held);
}
