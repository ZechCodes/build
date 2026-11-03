// Explicit mounted-content admission, shared with the cache-only index feed.
// This registry reads no cache or transport and never discovers more demand.
const demands = new Map();
const listeners = new Set();
const snapshot = () => [...demands.values()];
const announce = () => listeners.forEach((listener) => listener(snapshot()));

export function admitTaskReferenceLookups(scope, numbers) {
  if (!Array.isArray(numbers) || numbers.length > 4 || !numbers.every((number) => Number.isSafeInteger(number) && number > 0)) {
    throw new Error("Expected at most four exact task reference numbers");
  }
  const token = {};
  demands.set(token, { scope: { deviceId: scope.deviceId, projectId: scope.projectId }, numbers: [...new Set(numbers)] });
  announce();
  return () => { if (demands.delete(token)) announce(); };
}

export function subscribeTaskReferenceDemand(listener) {
  listeners.add(listener);
  listener(snapshot());
  return () => listeners.delete(listener);
}
