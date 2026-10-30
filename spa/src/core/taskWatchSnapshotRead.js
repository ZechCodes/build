// Actual GET dispatch ownership. This first delta only opts the request in;
// returned snapshot JSON does not yet supply admitted family authority.
import { captureTaskContentRead } from "./taskContentAuthority.js";
import { captureDeviceGreeting, contextFor } from "./deviceContexts.js";
import { endedOpeningError } from "./bridgeEpoch.js";

const requests = new WeakMap();
const utf8 = new TextEncoder();

const validIdentity = (value) => typeof value === "string" && value.isWellFormed()
  && value.length > 0 && utf8.encode(value).byteLength <= 128;

function snapshotGreeting(address) {
  const greeting = captureDeviceGreeting(contextFor(address.deviceId));
  if (greeting?.capabilities?.writes?.epoch !== true
    || greeting.capabilities.tasks?.watchSnapshotGet !== true) return null;
  const epoch = greeting.epoch;
  return validIdentity(epoch.bridge_epoch) && validIdentity(epoch.journal_incarnation) ? greeting : null;
}

function validateRequest(address, call) {
  if (address?.kind !== "tracker-task" || !address.deviceId || !address.entityId || !address.sub
    || typeof call !== "function") throw new TypeError("Expected an addressed task GET owner");
}

export async function captureTaskGetRead({ address, call, accept = () => true }) {
  validateRequest(address, call);
  const greeting = snapshotGreeting(address);
  const contentRead = await captureTaskContentRead(address, { revalidate: "full" });
  const request = Object.freeze({});
  requests.set(request, { address: { ...address }, call, accept, greeting, dispatched: false });
  return { contentRead, request };
}

export async function dispatchTaskGetRead(request) {
  const captured = requests.get(request);
  if (!captured || captured.dispatched) throw new Error("Task GET owner was not captured or was already dispatched");
  if (!captured.accept() || captured.greeting && !captured.greeting.current()) {
    throw endedOpeningError("tasks.get", false);
  }
  captured.dispatched = true;
  const params = { task_id: captured.address.sub, ...(captured.greeting ? { watch_snapshot: true } : {}) };
  const answer = await captured.call("tasks.get", params);
  // Proof parsing/publication is a separate test-first delta. Opt-in alone is
  // not permission to certify a row, retire a layer, or change a watch owner.
  return { answer, watchSnapshotRead: null };
}
