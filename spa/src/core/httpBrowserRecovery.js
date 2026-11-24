import { createBridgeDiscovery } from "./httpDiscovery.js";
import { httpFailure } from "./httpEndpoint.js";
import { checkHttpNetworkPermission } from "./httpNetworkPermission.js";

const startedAt = Date.now();
let navigating = false;

function documentAllows(origin) {
  const content = document.querySelector('meta[name="build-bridge-http-origins"]')?.content;
  if (!content) return true;
  try { return JSON.parse(content).includes(origin); } catch { return true; }
}

function challengeOnDocument(deviceId) {
  const url = new URL(location.href);
  return url.searchParams.get("bridge_recovery") === deviceId
    ? url.searchParams.get("bridge_discovery") : null;
}

async function recoverGrant(discovery, identity) {
  const challenge = challengeOnDocument(identity.deviceId);
  if (challenge) {
    const grant = await discovery.resumeSavedChallenge(challenge, identity).catch((error) => {
      // The reload might have taken past the challenge's lifetime, or the
      // page might have been restored after that challenge was consumed.
      if (error.code !== "authentication") throw error;
      return null;
    });
    if (grant) return grant;
  }
  return discovery.finishSaved(await discovery.beginSaved(identity), identity);
}

async function refreshDocument(discovery, identity, devices, current) {
  if (challengeOnDocument(identity.deviceId) && Date.now() - startedAt < 60_000) throw httpFailure("network");
  const pending = await discovery.beginSaved(identity);
  if (!current() || navigating) throw httpFailure("scope_ended");
  const target = new URL(pending.resume_url, location.origin);
  target.searchParams.set("bridge_recovery", identity.deviceId);
  target.searchParams.set("bridge_devices", devices.join(","));
  target.hash = location.hash;
  navigating = true;
  location.replace(target.href);
  throw httpFailure("scope_ended");
}

export async function openSavedBrowserGrant({ identity, accountId, devices, current }) {
  const discovery = createBridgeDiscovery({ issuer: location.origin, accountId, isCurrent: current });
  if (!documentAllows(identity.origin)) return refreshDocument(discovery, identity, devices, current);
  await checkHttpNetworkPermission(identity.origin);
  if (identity.familyId) {
    const grant = await discovery.resume(identity).catch((error) => {
      if (![401, 403].includes(error.status)) throw error;
      return null;
    });
    if (grant) return grant;
  }
  return recoverGrant(discovery, identity);
}
