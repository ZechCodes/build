// The lifecycle of attempts to connect one device. Device availability remains
// a projection owned by deviceContexts until the next refactor stage: a
// succeeded attempt here means only that this attempt completed authoritatively.

const CANCELLED = "connection attempt cancelled";

export function createDeviceConnectionAttempts() {
  const owners = new Map();

  return {
    isConnecting(deviceId) {
      return owners.get(deviceId)?.connecting === true;
    },

    forDevice(deviceId) {
      let owner = owners.get(deviceId);
      if (!owner) {
        owner = createOwner(deviceId);
        owners.set(deviceId, owner);
      }
      return owner;
    },

    retire(deviceId) {
      const owner = owners.get(deviceId);
      if (!owner) return;
      owners.delete(deviceId);
      owner.retire();
    },

    clear() {
      const captured = [...owners.values()];
      owners.clear();
      for (const owner of captured) owner.retire();
    },
  };
}

function createOwner(deviceId) {
  let state = "idle";
  let active = null;
  let authority = null;

  const transition = (event, attempt = active) => {
    if (event === "START") state = "connecting";
    if (event === "SUCCEED" && active === attempt) state = "succeeded";
    if (event === "FAIL" && active === attempt) state = "failed";
    if (event === "CANCEL" && active === attempt) state = "idle";
    if (event === "RETIRE") state = "retired";
  };

  const invalidate = (event) => {
    const attempt = authority;
    if (event === "RETIRE" || attempt) transition(event, active);
    if (!attempt) return;
    active = null;
    authority = null;
    attempt.invalidate(event === "RETIRE" ? "retired" : CANCELLED);
  };

  return {
    get state() {
      return state;
    },

    get connecting() {
      return state === "connecting";
    },

    connect(run, { onFailure = () => {} } = {}) {
      if (state === "retired") throw new Error(`connection-attempt owner for ${deviceId} is retired`);
      if (active) return active.promise;
      const previousAuthority = authority;
      const attempt = createAttempt();
      active = attempt;
      authority = attempt;
      transition("START", attempt);
      previousAuthority?.invalidate("superseded");
      attempt.start(run, {
        succeed: () => transition("SUCCEED", attempt),
        fail: (error) => {
          if (authority !== attempt) return;
          transition("FAIL", attempt);
          active = null;
          authority = null;
          attempt.fail();
          // A disposer may synchronously start the replacement. Its START is
          // newer authority, so the old failure must not block its device.
          if (!authority && state === "failed") onFailure(error);
        },
        finish: () => {
          if (active === attempt) active = null;
        },
      });
      return attempt.promise;
    },

    cancel() {
      invalidate("CANCEL");
    },

    retire() {
      if (state === "retired") return;
      invalidate("RETIRE");
    },
  };
}

function createAttempt() {
  let valid = true;
  const resources = new Map();
  let resolvePromise;
  let rejectPromise;
  const promise = new Promise((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });

  const token = {
    isCurrent: () => valid,
    own(resource, close) {
      if (!valid) {
        try {
          close(resource);
        } catch {
          // Cancellation already won; cleanup errors have no new authority.
        }
        return false;
      }
      resources.set(resource, close);
      return true;
    },
    release(resource) {
      return resources.delete(resource);
    },
  };

  const cleanResources = () => {
    const owned = [...resources.entries()];
    resources.clear();
    for (const [resource, close] of owned) {
      try {
        close(resource);
      } catch {
        // One broken disposer must not strand the resources after it.
      }
    }
  };

  return {
    promise,
    invalidate(reason) {
      if (!valid) return;
      valid = false;
      cleanResources();
      rejectPromise(new Error(reason));
    },
    fail() {
      if (!valid) return;
      valid = false;
      cleanResources();
    },
    start(run, events) {
      if (!valid) return;
      let result;
      try {
        result = run(token);
      } catch (error) {
        result = Promise.reject(error);
      }
      Promise.resolve(result).then(
        (value) => {
          if (!valid) return;
          events.succeed();
          events.finish();
          resolvePromise(value);
        },
        (error) => {
          if (!valid) return;
          try {
            events.fail(error);
          } catch {
            // Reporting cannot replace the connection error callers observed.
          }
          rejectPromise(error);
        },
      );
    },
  };
}
