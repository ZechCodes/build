// Discrete moments on a continuous clock. The scrubbed timeline runs both
// ways and can jump several acts in one frame, so a gate fires once for each
// crossing, in order: `forward` as the playhead passes it going up, `backward`
// as it passes going down. init() brings the gates at or before a time to
// their forward state without motion, for a page that opens mid-film.
export function createGates() {
  const gates = [];
  let last = null;
  return {
    add(time, forward, backward) {
      gates.push({ time, forward, backward });
      gates.sort((a, b) => a.time - b.time);
    },
    init(time) {
      last = time;
      for (const gate of gates) if (gate.time <= time) gate.forward(true);
    },
    update(time) {
      if (last === null) {
        this.init(time);
        return;
      }
      if (time > last) {
        for (const gate of gates) if (gate.time > last && gate.time <= time) gate.forward(false);
      } else if (time < last) {
        for (let index = gates.length - 1; index >= 0; index -= 1) {
          const gate = gates[index];
          if (gate.time <= last && gate.time > time) gate.backward?.(false);
        }
      }
      last = time;
    },
    get time() { return last; },
  };
}
