// Epochs are opaque process identities. Only a current session's greeting can
// establish one; an unexpected frame asks for a greeting and proves no order.
export function createBridgeEpoch() {
  let current = null;
  const changes = new Set();
  const unexpected = new Set();
  return {
    current: () => current,
    acceptGreeting(greeting, { isCurrent = () => true } = {}) {
      if (!isCurrent()) return false;
      if (!hasEpochCapability(greeting)) return true;
      const previous = current;
      const changed = previous?.bridge_epoch !== greeting.bridge_epoch;
      current = greetingSnapshot(greeting, previous, changed);
      if (changed) for (const listener of [...changes]) listener(current, previous);
      return true;
    },
    accepts(payload) {
      if (current && payload?.bridge_epoch === current.bridge_epoch) return true;
      for (const listener of [...unexpected]) listener(payload?.bridge_epoch);
      return false;
    },
    onChange: (fn) => subscribe(changes, fn),
    onUnexpected: (fn) => subscribe(unexpected, fn),
  };
}

function greetingSnapshot(greeting, previous, changed) {
  if (!validIdentity(greeting.bridge_epoch) || !validIdentity(greeting.journal_incarnation)) {
    throw new Error("The bridge greeting has an invalid epoch or journal incarnation");
  }
  return Object.freeze({
    bridge_epoch: greeting.bridge_epoch,
    journal_incarnation: greeting.journal_incarnation,
    ack_confirm_methods: Object.freeze([...(greeting.ack_confirm_methods || [])]),
    opening: (previous?.opening || 0) + Number(changed),
  });
}

const validIdentity = (value) => typeof value === "string" && value.length > 0;
const hasEpochCapability = (greeting) => Array.isArray(greeting?.capabilities) && greeting.capabilities.includes("bridge.epoch");
const subscribe = (listeners, fn) => {
  listeners.add(fn);
  return () => listeners.delete(fn);
};

const declarativeControls = new Set([
  "changes.subscribe", "changes.unsubscribe", "writes.subscribe",
  "term.attach", "agent.attach", "term.resize", "term.ack",
]);

/** This is caller settlement, never authorization to resend old input. Desired
 * subscriptions/screens can be re-established against their current identity. */
export function endedOpeningError(method, handedOff) {
  const code = handedOff ? "uncertain_delivery" : "scope_ended";
  return Object.assign(new Error(handedOff ? "Delivery on the previous bridge opening is uncertain" : "The bridge opening ended before sending"), {
    code,
    error_code: code,
    uncertain: handedOff,
    retryable: declarativeControls.has(method),
  });
}
