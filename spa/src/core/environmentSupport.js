// Rendering remembers the latest greeting; dispatch requires the live capability.
import { mergeCachedAtomically, readCached } from "./localCache.js";

const FLAGS = ["list", "createProject", "create", "addedSources", "archive", "rename", "secrets"];
const supportOf = (capabilities) => Object.fromEntries(FLAGS.map((flag) => [flag, capabilities?.[flag] === true]));

export const environmentSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: "environment-support" });

// Missing flags are not negative evidence: only a complete cached greeting
// without any environment capability identifies a pre-environment device.
export const predatesEnvironments = (support) => FLAGS.every(flag => support?.[flag] === false);

export async function readEnvironmentSupportRecord(deviceId = "") {
  return (await readCached(environmentSupportAddress(deviceId)))?.value ?? null;
}

export function rememberEnvironmentSupport(deviceId, capabilities, current = () => true) {
  if (!deviceId) return Promise.resolve(false);
  const support = supportOf(capabilities?.environments);
  return mergeCachedAtomically(environmentSupportAddress(deviceId), (held) =>
    current() && FLAGS.some((flag) => held?.[flag] !== support[flag]) ? support : null);
}

export async function readEnvironmentSupport(deviceId) {
  const held = deviceId ? await readEnvironmentSupportRecord(deviceId) : null;
  return supportOf(held);
}
