// Account-scoped saved hints and in-memory prepared grants. Only discovery or
// hosted renewal can turn a persisted address back into an authenticated wire.
import { App } from "../appState.js";
import { writeAccountId } from "./writeOperationRuntime.js";
import { readUiRecord, subscribeUiRecords } from "./localUiStore.js";
import { updateUiState, uiAddress } from "./localUiState.js";
import { createHttpConnectionPreferences, httpRecoveryDevices, sameHttpDevice } from "./httpConnectionPreferences.js";
import { createBridgeDiscovery } from "./httpDiscovery.js";
import { createBrowserHttpEndpoint } from "./httpBrowserEndpoint.js";
import { openSavedBrowserGrant } from "./httpBrowserRecovery.js";
import { createDesktopHttpEndpoint, desktopHttpValue } from "./httpDesktopEndpoint.js";
import { createHttpRoutes } from "./httpRoutes.js";
import { canonicalBridgeOrigin, httpFailure } from "./httpEndpoint.js";
import { selectDeviceTransport } from "./deviceTransportSelection.js";

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

async function persistNativePreference(accountId, deviceId, replacement, expected, current) {
  const api = native();
  if (!api?.settingsWrite || expected && (expected.fingerprint || !replacement?.fingerprint)) return true;
  if (!current()) throw httpFailure("scope_ended");
  return desktopHttpValue(await api.settingsWrite({
    accountId, deviceId,
    setting: replacement ? settingFields(replacement) : null,
    ...(expected && { expected: settingFields(expected) }),
  }));
}

async function writePreference(address, value, { deviceId, expected }, current) {
  const replacement = value.connections.find((row) => row.deviceId === deviceId);
  if (!await persistNativePreference(value.accountId, deviceId, replacement, expected, current)) return false;
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
export const httpPreferenceFor = (deviceId) =>
  runtime?.current() ? runtime.preferences.forDevice(deviceId) : null;

export function deviceTransportFor(deviceId) {
  const policy = App.deviceTransportPolicy;
  if (policy && policy.accountId !== writeAccountId()) return selectDeviceTransport(null, null, true);
  const device = App.devices.find((row) => row.id === deviceId);
  const policyAware = policy?.policyAware ?? App.devices.some((row) => Object.hasOwn(row, "transport"));
  return selectDeviceTransport(device, httpPreferenceFor(deviceId), policyAware);
}

export function cachedTerminalTransport(deviceId) {
  const selection = deviceTransportFor(deviceId);
  if (selection.kind !== "http") return undefined;
  if (!selection.wssOnly) return runtime?.preferences.terminalModeFor(deviceId);
  const cached = runtime?.current() && runtime.terminalModes.get(deviceId);
  return cached?.key === selection.key ? cached.mode : "unknown";
}

export const usesHttp = (deviceId) => deviceTransportFor(deviceId).kind === "http";
export const httpRecoveryCandidates = (devices) => httpRecoveryDevices(devices, usesHttp);

export async function restoreHttpConnections() {
  const accountId = writeAccountId();
  if (!accountId) return;
  if (runtime?.accountId === accountId && runtime.epoch === App.accountEpoch) return runtime.ready;
  resetHttpConnections();
  const epoch = App.accountEpoch;
  let held;
  const current = () => runtime === held && App.accountEpoch === epoch && writeAccountId() === accountId;
  const address = uiAddress({ entityId: accountId, view: "account", kind: "http-connections" });
  const preferences = createHttpConnectionPreferences({ accountId, isCurrent: current,
    read: () => readPreference(address, accountId, current),
    write: (value, change) => writePreference(address, value, change, current),
  });
  const discovery = createBridgeDiscovery({ issuer: location.origin, accountId, isCurrent: current });
  const terminalAddress = uiAddress({ entityId: accountId, view: "account", kind: "projected-http-terminal" });
  held = { accountId, epoch, current, preferences, discovery, terminalAddress,
    projected: new Map(), reverify: new WeakSet(), terminalModes: new Map() };
  runtime = held;
  held.stopTerminalWatch = subscribeUiRecords(terminalAddress, () => {
    void readTerminalModes(held).then(() => { if (current()) announce(); });
  });
  held.ready = Promise.all([preferences.restore(), readTerminalModes(held)]);
  await held.ready;
  if (current()) announce();
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

async function readTerminalModes(held) {
  const modes = (await readUiRecord(held.terminalAddress))?.value;
  if (held.current()) held.terminalModes = new Map(Array.isArray(modes) ? modes : []);
}

async function rememberProjectedTerminalMode(held, deviceId, selection, mode, active) {
  const cached = held.terminalModes.get(deviceId);
  if (cached?.key === selection.key && cached.mode === mode) return false;
  const applied = await updateUiState(held.terminalAddress, (previous) => {
    const next = new Map(previous || []);
    next.set(deviceId, { key: selection.key, mode });
    return [...next];
  }, active);
  if (applied) await readTerminalModes(held);
  return applied;
}

export async function rememberHttpTerminalMode(deviceId, mode, current, selection = deviceTransportFor(deviceId)) {
  const held = runtime;
  if (!held?.current()) return;
  const active = () => held.current() && current() && deviceTransportFor(deviceId).key === selection.key;
  if (selection.wssOnly) {
    if (await rememberProjectedTerminalMode(held, deviceId, selection, mode, active) && active()) announce();
    return;
  }
  const preference = held.preferences.forDevice(deviceId);
  if (preference && await held.preferences.rememberTerminalMode(preference, mode, active) && active()) announce();
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

export function retireHttpSelection(deviceId) {
  discard(prepared.get(deviceId));
  prepared.delete(deviceId);
  runtime?.projected.delete(deviceId);
  for (const endpoint of endpoints) {
    if (endpoint.identity.deviceId === deviceId) endpoint.close();
  }
}

export async function forgetHttpConnection(deviceId) {
  retireHttpSelection(deviceId);
  if (runtime?.current()) await runtime.preferences.forget(deviceId);
  announce();
}

export function discardHttpDiscovery(identity) {
  if (prepared.get(identity.deviceId)?.identity !== identity) return;
  discard(prepared.get(identity.deviceId)); prepared.delete(identity.deviceId);
}

function projectedPreference(held, deviceId, selection) {
  const saved = held.projected.get(deviceId);
  if (saved?.key === selection.key) return saved.preference;
  const preference = { deviceId, origin: selection.origin, fingerprint: selection.fingerprint };
  held.projected.set(deviceId, { key: selection.key, preference });
  return preference;
}

export async function httpEndpointFor(deviceId, isCurrent, selection = deviceTransportFor(deviceId)) {
  const held = accountRuntime();
  if (selection.kind !== "http") throw httpFailure("scope_ended");
  let preference = selection.wssOnly
    ? projectedPreference(held, deviceId, selection) : held.preferences.forDevice(deviceId);
  const activePreference = () => selection.wssOnly
    ? held.projected.get(deviceId)?.preference : held.preferences.forDevice(deviceId);
  const current = () => held.current() && isCurrent() && deviceTransportFor(deviceId).key === selection.key
    && activePreference() === preference;
  const remember = async (identity) => {
    if (!current()) throw httpFailure("scope_ended");
    if (!sameHttpDevice(preference, identity)) throw httpFailure("wrong_bridge");
    if (matchesPreference(identity, preference) && !(identity.fingerprint && !preference.fingerprint)) return;
    const next = selection.wssOnly ? rememberProjectedGrant(held, selection, identity)
      : await held.preferences.rememberAuthority(preference, identity, current);
    if (!next) throw httpFailure("scope_ended");
    preference = next;
  };
  return openHttpEndpoint(held, preference, current, remember, () => preference, selection.wssOnly);
}

function rememberProjectedGrant(held, selection, { deviceId, origin, fingerprint, familyId, endpointEpoch }) {
  const preference = { deviceId, origin, fingerprint, familyId, endpointEpoch };
  held.projected.set(deviceId, { key: selection.key, preference });
  return preference;
}

async function openHttpEndpoint(held, preference, current, remember, latestPreference, wssOnly) {
  const api = native();
  if (wssOnly && !api && /\bElectron\//.test(navigator.userAgent)) throw httpFailure("desktop_update_required");
  const deviceId = preference.deviceId;
  const candidate = prepared.get(deviceId);
  const ready = matchesPreference(candidate?.identity, preference) ? candidate : null;
  if (ready) prepared.delete(deviceId);
  const hint = held.reverify.has(preference) ? { ...preference, familyId: undefined } : preference;
  const endpoint = api
    ? await nativeEndpoint(api, ready, hint, current, remember, wssOnly)
    : await browserEndpoint(held, ready, hint, current, remember);
  if (!current()) { endpoint.close(); throw httpFailure("scope_ended"); }
  const open = endpoint.open;
  endpoint.open = async (range) => {
    try {
      const opening = await open(range);
      held.reverify.delete(latestPreference());
      return opening;
    } catch (error) {
      if (error.code === "opening_refused" && current()) held.reverify.add(latestPreference());
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

async function nativeSavedIdentity(api, { deviceId, origin, endpointEpoch, familyId, fingerprint }, current, wssOnly) {
  const pin = fingerprint && { fingerprint };
  if (familyId) {
    const result = await api.resume({ deviceId, origin, endpointEpoch, familyId, ...pin });
    if (result?.error?.code !== "unauthorized") return desktopHttpValue(result);
  }
  if (!current()) throw httpFailure("scope_ended");
  if (!api.discoverSaved) throw httpFailure(wssOnly ? "desktop_update_required" : "incompatible_transport");
  return desktopHttpValue(await api.discoverSaved(origin, { deviceId, ...pin }));
}

async function nativeEndpoint(api, ready, preference, current, remember, wssOnly) {
  const identity = ready?.identity || await nativeSavedIdentity(api, preference, current, wssOnly);
  try { await remember(identity); }
  catch (error) { void api.close(identity.handle); throw error; }
  return createDesktopHttpEndpoint({ api, identity, isCurrent: current });
}

async function browserEndpoint(held, ready, preference, current, remember) {
  const devices = App.devices.map((row) => row.id).filter(usesHttp);
  if (!devices.includes(preference.deviceId)) devices.unshift(preference.deviceId);
  const grant = ready?.grant || await openSavedBrowserGrant({
    identity: preference, accountId: held.accountId, devices, current,
  });
  try { await remember(grant.identity); }
  catch (error) { grant.close(); throw error; }
  return createBrowserHttpEndpoint({ grant, isCurrent: current });
}

export function resetHttpConnections() {
  runtime?.stopTerminalWatch();
  for (const endpoint of [...endpoints]) endpoint.close();
  for (const item of prepared.values()) discard(item);
  prepared.clear();
  runtime = null;
}
