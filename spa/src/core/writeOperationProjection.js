// Accepted layers are local evidence. Canonical records remain independent so
// incoming pushes can rebase them and one refused write removes only its layer.
import { recordKey } from "./idbRecords.js";

export const WRITE_OPERATION_STORE = "write-operations-v1";
export const WRITE_LEASE_STORE = "write-leases-v1";
const activeScopes = new Map();
const scopeParts = ({ accountId, deviceId, principalId }) => [accountId, deviceId, principalId];
export const operationNamespace = (scope) => JSON.stringify(scopeParts(scope));
export const operationKey = (scope, operationId) => JSON.stringify([...scopeParts(scope), operationId]);
export const leaseKey = ({ accountId, deviceId }) => JSON.stringify([accountId, deviceId]);

export const clearOperationProjections = () => activeScopes.clear();
export const activeOperationNamespaces = (addresses) => [...new Set(addresses.map((address) =>
  activeScopes.get(address.deviceId)?.namespace).filter(Boolean))];

export function activateOperationProjection(scope, owner) {
  activeScopes.set(scope.deviceId, { namespace: operationNamespace(scope), owner });
}
export function deactivateOperationProjection(scope, owner) {
  if (activeScopes.get(scope.deviceId)?.owner === owner) activeScopes.delete(scope.deviceId);
}

const nonnegativeInteger = (value) => Number.isSafeInteger(value) && value >= 0;
export const validPublicationFence = (fence) => Boolean(fence && typeof fence.kind === "string" &&
  typeof fence.id === "string" && nonnegativeInteger(fence.generation) && nonnegativeInteger(fence.sequence));
const matchingFence = (left, right) => validPublicationFence(left) && validPublicationFence(right) &&
  left.kind === right.kind && left.id === right.id && left.generation === right.generation;
export function publicationCovers(observed, required) {
  return required.length > 0 && required.every((fence) => observed.some((seen) =>
    matchingFence(seen, fence) && Number.isSafeInteger(seen.sequence) && seen.sequence >= fence.sequence));
}

export function requiredPublication(operation) {
  return operation.confirmation?.published || operation.published || [];
}

export function layerPublished(operation, records) {
  if (operation.state !== "succeeded") return false;
  const required = requiredPublication(operation);
  const effects = operation.overlay || [];
  return effects.length > 0 && effects.every(({ address }) =>
    publicationCovers(records.get(recordKey(address))?.publication || [], required));
}

const valueAtPath = (value, path) => path.reduce((held, part) => held?.[part], value);

function applyValue(previous, effect) {
  if (effect.kind === "replace") return effect.value;
  if (effect.kind === "merge") return { ...previous, ...effect.value };
  if (effect.kind !== "upsert") throw new Error(`Unsupported write layer: ${effect.kind}`);
  const rows = Array.isArray(previous) ? previous : [];
  const key = Array.isArray(effect.key) ? effect.key : [effect.key || "id"];
  const identity = valueAtPath(effect.value, key);
  const index = rows.findIndex((row) => identity !== undefined && valueAtPath(row, key) === identity);
  if (index === -1) return [...rows, effect.value];
  if (effect.insertOnly) return rows;
  return rows.map((row, position) => position === index ? { ...row, ...effect.value } : row);
}

function applyPath(previous, path, effect) {
  if (!path.length) return applyValue(previous, effect);
  const [head, ...rest] = path;
  if (["__proto__", "prototype", "constructor"].includes(head)) throw new Error("Unsafe write layer path");
  return { ...previous, [head]: applyPath(previous?.[head], rest, effect) };
}

function compareLayers(left, right) {
  for (const owner of left.owners || []) {
    const shared = (right.owners || []).find((other) => matchingFence(owner, other));
    if (shared && owner.sequence !== shared.sequence) return owner.sequence - shared.sequence;
  }
  return left.createdAt - right.createdAt || left.localOrder - right.localOrder || left.operationId.localeCompare(right.operationId);
}

export function composeOperationRecords(addresses, canonical, operations) {
  const byKey = new Map(addresses.map((address, index) => [recordKey(address), canonical[index]]));
  const layers = operations.filter((operation) => operation.overlayActive &&
    activeScopes.get(operation.deviceId)?.namespace === operation.namespace && !layerPublished(operation, byKey));
  layers.sort(compareLayers);
  for (const operation of layers) {
    for (const effect of operation.overlay || []) {
      const key = recordKey(effect.address);
      if (!byKey.has(key)) continue;
      const previous = byKey.get(key);
      byKey.set(key, { ...previous, value: applyPath(previous?.value, effect.path || [], effect), accepted: true });
    }
  }
  return addresses.map((address) => byKey.get(recordKey(address)));
}
