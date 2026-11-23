// Account-scoped saved hints and in-memory prepared grants. Only discovery or
// hosted renewal can turn a persisted address back into an authenticated wire.
import { App } from "../appState.js";
import { writeAccountId } from "./writeOperationRuntime.js";
import { readUiRecord } from "./localUiStore.js";
import { updateUiState, uiAddress } from "./localUiState.js";
import { createHttpConnectionPreferences, httpRecoveryDevices, sameHttpDevice } from "./httpConnectionPreferences.js";
import { createBridgeDiscovery } from "./httpDiscovery.js";
import { createBrowserHttpEndpoint } from "./httpBrowserEndpoint.js";
import { openSavedBrowserGrant } from "./httpBrowserRecovery.js";
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

const settingFields = ({ deviceId, origin, selected, fingerprint }) => ({
  deviceId, origin, selected, ...(fingerprint && { fingerprint }),
});
const accountConnections = (record, accountId) =>
  record?.accountId === accountId && Array.isArray(record.connections) ? record.connections : [];

async function readPreference(address, accountId, current) {
  const cache = (await readUiRecord(address))?.value;
  const api = native();
  if (!api?.settingsRead) return cache;
  const saved = desktopHttpValue(await api.settingsRead());
  const local = accountConnections(cache, accountId).filter((row) => row?.deviceId);
  if (!current()) throw httpFailure("scope_ended");
  if (!saved) {
    for (const row of local) {
      const setting = settingFields(row);
      desktopHttpValue(await api.settingsWrite({ accountId, deviceId: setting.deviceId, setting }));
    }
    return cache;
  }
  if (saved.accountId !== accountId) throw httpFailure("sign_in");
  const value = { accountId, connections: saved.connections.map((setting) =>
    local.find((row) => row.deviceId === setting.deviceId && sameStoredSetting(row, setting)) || setting) };
  await updateUiState(address, () => value, current);
  return value;
}

async function writePreference(address, value, { deviceId, expected }, current) {
  const replacement = value.connections.find((row) => row.deviceId === deviceId);
  const api = native();
  if (!expected && api?.settingsWrite) {
    if (!current()) throw httpFailure("scope_ended");
    desktopHttpValue(await api.settingsWrite({
      accountId: value.accountId, deviceId, setting: replacement ? settingFields(replacement) : null,
    }));
  }
  let committed = false;
  const applied = await updateUiState(address, (previous) => {
    const saved = accountConnections(previous, value.accountId);
    const old = saved.find((row) => row?.deviceId === deviceId);
    if (expected && !sameStoredSetting(old, expected)) return previous;
    const retained = saved.filter((row) => row?.deviceId !== deviceId);
    if (replacement) retained.push(replacement);
    if (retained.length > 16) return previous;
    committed = true;
    return { accountId: value.accountId, connections: retained };
  }, current);
  return committed && applied;
}

function sameStoredSetting(stored, expected) {
  return stored?.selected === expected.selected && stored.origin === expected.origin
    && stored.fingerprint === expected.fingerprint;
}

export function configureHttpManifest(manifest) { routes = createHttpRoutes(manifest); }
export const httpResourceRoutes = () => { if (!routes) throw httpFailure("unsupported"); return routes; };
export const onHttpPreferencesChanged = (listener) => { listeners.add(listener); return () => listeners.delete(listener); };
export const httpPreferenceFor = (deviceId) => runtime?.preferences.forDevice(deviceId) || null;
export const cachedTerminalTransport = (deviceId) =>
  runtime?.current() ? runtime.preferences.terminalModeFor(deviceId) : undefined;
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
    read: () => readPreference(address, accountId, current),
    write: (value, change) => writePreference(address, value, change, current),
  });
  const discovery = createBridgeDiscovery({ issuer: location.origin, accountId, isCurrent: current });
  runtime = { accountId, epoch, current, preferences, discovery, reverify: new WeakSet(), ready: preferences.restore() };
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

export async function rememberHttpTerminalMode(deviceId, mode, current) {
  const held = runtime;
  const preference = held?.preferences.forDevice(deviceId);
  if (!preference || !held.current()) return;
  const changed = await held.preferences.rememberTerminalMode(preference, mode, current);
  if (changed && held.current()) announce();
}

export async function selectHttpConnection(identity) {
  if (!await accountRuntime().preferences.selectHttp(identity)) throw httpFailure("busy");
  announce();
}

export async function selectWebRtcConnection(deviceId) {
  if (!await accountRuntime().preferences.selectWebRtc(deviceId)) throw httpFailure("busy");
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
  let preference = held.preferences.forDevice(deviceId);
  if (preference?.selected !== "http") throw httpFailure("scope_ended");
  const current = () => held.current() && isCurrent() && held.preferences.forDevice(deviceId) === preference;
  const remember = async (identity) => {
    if (!current()) throw httpFailure("scope_ended");
    if (!sameHttpDevice(preference, identity)) throw httpFailure("wrong_bridge");
    if (matchesPreference(identity, preference)) return;
    const next = await held.preferences.rememberAuthority(preference, identity, current);
    if (!next) throw httpFailure("scope_ended");
    preference = next;
  };
  const candidate = prepared.get(deviceId);
  const ready = matchesPreference(candidate?.identity, preference) ? candidate : null;
  if (ready) prepared.delete(deviceId);
  const hint = held.reverify.has(preference) ? { ...preference, familyId: undefined } : preference;
  const api = native();
  const endpoint = api
    ? await nativeEndpoint(api, ready, hint, current, remember)
    : await browserEndpoint(held, ready, hint, current, remember);
  const open = endpoint.open;
  endpoint.open = async (range) => {
    try {
      const opening = await open(range);
      held.reverify.delete(preference);
      return opening;
    } catch (error) {
      if (error.code === "opening_refused" && current()) held.reverify.add(preference);
      throw error;
    }
  };
  endpoints.add(endpoint);
  const close = endpoint.close;
  endpoint.close = () => { endpoints.delete(endpoint); close(); };
  return endpoint;
}

const matchesPreference = (identity, preference) => identity && sameHttpDevice(preference, identity)
  && identity.familyId === preference.familyId && identity.endpointEpoch === preference.endpointEpoch;

async function nativeSavedIdentity(api, { deviceId, origin, endpointEpoch, familyId, fingerprint }, current) {
  if (familyId) {
    const result = await api.resume({ deviceId, origin, endpointEpoch, familyId });
    if (result?.ok || result?.error?.code !== "unauthorized") return desktopHttpValue(result);
  }
  if (!current()) throw httpFailure("scope_ended");
  if (!api.discoverSaved) throw httpFailure("incompatible_transport");
  return desktopHttpValue(await api.discoverSaved(origin, { deviceId, ...(fingerprint && { fingerprint }) }));
}

async function nativeEndpoint(api, ready, preference, current, remember) {
  const identity = ready?.identity || await nativeSavedIdentity(api, preference, current);
  try { await remember(identity); }
  catch (error) { void api.close(identity.handle); throw error; }
  return createDesktopHttpEndpoint({ api, identity, isCurrent: current });
}

async function browserEndpoint(held, ready, preference, current, remember) {
  const devices = held.preferences.all().filter((item) => item.selected === "http").map((item) => item.deviceId);
  const grant = ready?.grant || await openSavedBrowserGrant({
    identity: preference, accountId: held.accountId, devices, current,
  });
  try { await remember(grant.identity); }
  catch (error) { grant.close(); throw error; }
  return createBrowserHttpEndpoint({ grant, isCurrent: current });
}

export function resetHttpConnections() {
  for (const endpoint of [...endpoints]) endpoint.close();
  for (const item of prepared.values()) discard(item);
  prepared.clear();
  runtime = null;
}
