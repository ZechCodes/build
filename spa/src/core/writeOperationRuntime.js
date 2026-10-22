import { createWriteOperationCache } from "./writeOperationCache.js";
import { createWriteOperationService } from "./writeOperations.js";
import { normalizeAckEffects, normalizeIntentEffects } from "./writeOperationEffects.js";
import { refreshWriteOperation } from "./writeOperationRefresh.js";

const services = new Map();
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** Identity comes from the authenticated document, never storage or the URL.
 * A development shell or signed-out page has no authority over pending writes. */
export function writeAccountId() {
  const value = globalThis.document?.querySelector('meta[name="build-account-id"]')?.content;
  return UUID.test(value || "") ? value.toLowerCase() : null;
}

export function deviceWriteOperations(deviceId) {
  const accountId = writeAccountId();
  if (!accountId || !deviceId) return null;
  const key = JSON.stringify([accountId, deviceId]);
  if (!services.has(key)) {
    const identity = { accountId, deviceId, principalId: `user:${accountId}` };
    const cache = createWriteOperationCache(identity);
    services.set(key, createWriteOperationService({
      ...identity, cache, normalizeAckEffects, normalizeIntentEffects,
      onRefresh: (addresses, frame, record, authority) => refreshWriteOperation({ cache, addresses, frame, record, authority }),
    }));
  }
  return services.get(key);
}

/** Called before cache readers mount. Greeting controls dispatch only. */
export async function restoreWriteOperations(devices) {
  await Promise.all(devices.map(async ({ id }) => {
    const service = deviceWriteOperations(id);
    try { await service?.list(); }
    catch { /* Storage refusal is surfaced by the action; cached paint remains available. */ }
  }));
}

export async function resetWriteOperationRuntime() {
  const held = [...services.values()];
  services.clear();
  await Promise.all(held.map((service) => service.close()));
}

export function dispatchWriteConfirmation(frame, deviceId) {
  const service = deviceWriteOperations(deviceId);
  if (service) void service.handleFrame(frame).catch(() => {});
}
