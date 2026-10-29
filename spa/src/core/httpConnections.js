// Account-scoped saved hints and in-memory prepared grants. Only discovery or
// hosted renewal can turn a persisted address back into an authenticated wire.
import { App } from "../appState.js";
import { writeAccountId } from "./writeOperationRuntime.js";
import { readUiRecord } from "./localUiStore.js";
import { updateUiState, uiAddress } from "./localUiState.js";
import { createHttpConnectionPreferences, httpRecoveryDevices } from "./httpConnectionPreferences.js";
import { createBridgeDiscovery } from "./httpDiscovery.js";
import { createBrowserHttpEndpoint } from "./httpBrowserEndpoint.js";
import { createDesktopHttpEndpoint, desktopHttpValue } from "./httpDesktopEndpoint.js";
import { createHttpRoutes } from "./httpRoutes.js";
import { canonicalBridgeOrigin, httpFailure } from "./httpEndpoint.js";

let runtime = null;
let routes = null;
const prepared = new Map();
const endpoints = new Set();
const listeners = new Set();
const native = () => globalThis.window?.buildBridgeHttp || null;
const announce = () => listeners.forEach((listener) => listener());
const discard = (item) => {
  item?.grant?.close();
  if (item?.handle) void native()?.close(item.handle);
};

export function configureHttpManifest(manifest) { routes = createHttpRoutes(manifest); }
export const httpResourceRoutes = () => { if (!routes) throw httpFailure("unsupported"); return routes; };
export const onHttpPreferencesChanged = (listener) => { listeners.add(listener); return () => listeners.delete(listener); };
export const httpPreferenceFor = (deviceId) => runtime?.preferences.forDevice(deviceId) || null;
export const usesHttp = (deviceId) => httpPreferenceFor(deviceId)?.selected === "http";
export const httpRecoveryCandidates = (devices) => httpRecoveryDevices(devices, httpPreferenceFor);

export async function restoreHttpConnections() {
  const accountId = writeAccountId();
  if (!accountId) return;
  if (runtime?.accountId === accountId && runtime.epoch === App.accountEpoch) return runtime.ready;
  resetHttpConnections();
  const epoch = App.accountEpoch;
  const current = () => App.accountEpoch === epoch && writeAccountId() === accountId;
  const address = uiAddress({ entityId: accountId, view: "account", kind: "http-connections" });
  const preferences = createHttpConnectionPreferences({ accountId, isCurrent: current,
    read: async () => (await readUiRecord(address))?.value,
    write: (value) => updateUiState(address, () => value, current),
  });
  const discovery = createBridgeDiscovery({ issuer: location.origin, accountId, isCurrent: current });
  runtime = { accountId, epoch, current, preferences, discovery, ready: preferences.restore() };
  await runtime.ready;
  announce();
}

const accountRuntime = () => {
  if (!runtime?.current()) throw httpFailure("authentication");
  return runtime;
};

export async function beginHttpDiscovery(input) {
  const origin = canonicalBridgeOrigin(input);
  httpResourceRoutes();
  await restoreHttpConnections();
  const held = accountRuntime();
  const api = native();
  if (!api) return { navigateTo: await held.discovery.begin(origin) };
  const identity = desktopHttpValue(await api.discover(origin));
  if (!held.current() || identity.origin !== origin) { void api.close(identity.handle); throw httpFailure("scope_ended"); }
  discard(prepared.get(identity.deviceId));
  prepared.set(identity.deviceId, { identity, handle: identity.handle });
  return { identity };
}

export async function completeHttpDiscovery(challengeId) {
  httpResourceRoutes();
  await restoreHttpConnections();
  const held = accountRuntime();
  const grant = await held.discovery.complete(challengeId);
  if (!held.current()) { grant.close(); throw httpFailure("scope_ended"); }
  discard(prepared.get(grant.identity.deviceId));
  prepared.set(grant.identity.deviceId, { identity: grant.identity, grant });
  return grant.identity;
}

export async function selectHttpConnection(identity) {
  await accountRuntime().preferences.selectHttp(identity);
  announce();
}

export async function selectWebRtcConnection(deviceId) {
  await accountRuntime().preferences.selectWebRtc(deviceId);
  discard(prepared.get(deviceId));
  prepared.delete(deviceId);
  announce();
}

export async function forgetHttpConnection(deviceId) {
  discard(prepared.get(deviceId));
  prepared.delete(deviceId);
  if (runtime?.current()) await runtime.preferences.forget(deviceId);
  announce();
}

export function discardHttpDiscovery(identity) {
  if (prepared.get(identity.deviceId)?.identity !== identity) return;
  discard(prepared.get(identity.deviceId)); prepared.delete(identity.deviceId);
}

export async function httpEndpointFor(deviceId, isCurrent) {
  const held = accountRuntime();
  const preference = held.preferences.forDevice(deviceId);
  if (preference?.selected !== "http") throw httpFailure("scope_ended");
  const current = () => held.current() && isCurrent() && held.preferences.forDevice(deviceId) === preference;
  const candidate = prepared.get(deviceId);
  const ready = candidate?.identity.familyId === preference.familyId ? candidate : null;
  if (ready) prepared.delete(deviceId);
  let endpoint;
  const api = native();
  if (api) {
    endpoint = await nativeEndpoint(api, ready, preference, current);
  } else {
    endpoint = await browserEndpoint(held, ready, preference, current);
  }
  endpoints.add(endpoint);
  const close = endpoint.close;
  endpoint.close = () => { endpoints.delete(endpoint); close(); };
  return endpoint;
}

async function nativeEndpoint(api, ready, preference, current) {
  const identity = ready?.identity || desktopHttpValue(await api.resume(preference));
  if (!current()) { void api.close(identity.handle); throw httpFailure("scope_ended"); }
  return createDesktopHttpEndpoint({ api, identity, isCurrent: current });
}

async function browserEndpoint(held, ready, preference, current) {
  const grant = ready?.grant || await held.discovery.resume(preference);
  if (!current()) { grant.close(); throw httpFailure("scope_ended"); }
  return createBrowserHttpEndpoint({ grant, isCurrent: current });
}

export function resetHttpConnections() {
  for (const endpoint of [...endpoints]) endpoint.close();
  for (const item of prepared.values()) discard(item);
  prepared.clear();
  runtime = null;
}
