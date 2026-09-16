// Ownership for per-device rendezvous sockets. A lease keeps the rendezvous
// open while one operation negotiates; the last release closes it. Forced
// closure invalidates the generation captured by live callbacks without
// retiring the owner, while retirement makes a captured owner unusable.

export function createRendezvousLifecycle(createRendezvous) {
  const owners = new Map();

  return {
    forDevice(deviceId) {
      let owner = owners.get(deviceId);
      if (!owner) {
        owner = createOwner(deviceId, createRendezvous);
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
      for (const owner of owners.values()) owner.retire();
      owners.clear();
    },
  };
}

function createOwner(deviceId, createRendezvous) {
  let state = "idle";
  let generation = 0;
  let rendezvous = null;
  const activeLeases = new Set();

  const closeActiveGeneration = (nextState) => {
    const shouldClose = state === "negotiating";
    generation += 1;
    activeLeases.clear();
    state = nextState;
    if (shouldClose) rendezvous.close();
  };

  const acquireForGeneration = (expectedGeneration) => {
    if (state === "retired" || expectedGeneration !== generation) return null;
    if (!rendezvous) rendezvous = createRendezvous(deviceId);
    state = "negotiating";
    const token = {};
    activeLeases.add(token);
    let released = false;
    return {
      rendezvous,
      release() {
        if (released) return;
        released = true;
        if (expectedGeneration !== generation || !activeLeases.delete(token)) return;
        if (activeLeases.size === 0) {
          state = "idle";
          rendezvous.close();
        }
      },
      reacquire() {
        return acquireForGeneration(expectedGeneration);
      },
    };
  };

  return {
    get state() {
      return state;
    },

    acquire() {
      if (state === "retired") throw new Error(`rendezvous owner for ${deviceId} is retired`);
      return acquireForGeneration(generation);
    },

    forceClose() {
      if (state !== "retired") closeActiveGeneration("idle");
    },

    retire() {
      if (state !== "retired") closeActiveGeneration("retired");
    },
  };
}
