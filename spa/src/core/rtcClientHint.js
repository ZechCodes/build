// A browser's LAN discovery hint belongs to one pairing, never to an account.
// Only the opaque UUID goes inside encrypted rtc.offer; it is not diagnostic.
import { capabilitiesOf } from "./bridgeApi/v1/index.js";

const PREFIX = "build.rtc-client:";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const devicePrefix = (deviceId) => `${PREFIX}${encodeURIComponent(deviceId)}:`;
const pairingKey = (deviceId, transportKey) => deviceId && transportKey
  ? `${devicePrefix(deviceId)}${encodeURIComponent(transportKey)}` : null;
const readPairing = (storage, key) => JSON.parse(storage.getItem(key) || "null");

/** Hello supplies the capability only after the first offer. The cached fact
 * permits a later offer or restart to send a hint to this same paired bridge. */
export function rememberRtcClientSupport(deviceId, transportKey, greeting) {
  const key = pairingKey(deviceId, transportKey);
  if (!key) return;
  try {
    const storage = globalThis.localStorage;
    const held = readPairing(storage, key);
    storage.setItem(key, JSON.stringify({
      supported: capabilitiesOf(greeting).rtc.clientLanCache,
      clientId: UUID.test(held?.clientId) ? held.clientId : null,
    }));
  } catch { /* browser storage is optional; unavailable means no client hint */ }
}

export function rtcClientId(deviceId, transportKey) {
  const key = pairingKey(deviceId, transportKey);
  if (!key) return null;
  try {
    const storage = globalThis.localStorage;
    const held = readPairing(storage, key);
    if (held?.supported !== true) return null;
    if (UUID.test(held.clientId)) return held.clientId;
    const clientId = globalThis.crypto?.randomUUID?.();
    if (!UUID.test(clientId)) return null;
    storage.setItem(key, JSON.stringify({ supported: true, clientId }));
    return clientId;
  } catch { return null; }
}

/** Unpairing forgets every key generation for that device. Account replacement
 * forgets all pairings while leaving unrelated browser preferences alone. */
export function forgetRtcClientHints(deviceId = null) {
  try {
    const storage = globalThis.localStorage;
    const prefix = deviceId ? devicePrefix(deviceId) : PREFIX;
    const keys = Array.from({ length: storage.length }, (_, index) => storage.key(index));
    for (const key of keys) if (key?.startsWith(prefix)) storage.removeItem(key);
  } catch { /* disabled storage has no usable hint to forget */ }
}
