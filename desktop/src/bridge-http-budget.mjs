import { MAX_JSON_BYTES } from "./bridge-http-network.mjs";
import { fail } from "./bridge-http-contract.mjs";

// Reservations cover encoded request/response maxima, not JS object/decoded heap.
export function createWireBudget() {
  let bytes = 0;
  let bulkBytes = 0;
  let bulkCount = 0;
  const bulkOpenings = new Set();
  const families = new Map();
  const openings = new Map();
  return {
    reserve({ requestMax = 0, responseMax = MAX_JSON_BYTES, opening, family } = {}) {
      const total = requestMax + responseMax;
      const bulk = requestMax > MAX_JSON_BYTES;
      // Full reservations trade a higher worst-case allowance for never losing a
      // reply after execution. Each family/opening can hold only half the pool.
      if (bytes + total > 288 * MAX_JSON_BYTES ||
          (families.get(family) || 0) + total > 144 * MAX_JSON_BYTES ||
          (openings.get(opening) || 0) + total > 144 * MAX_JSON_BYTES) fail("busy");
      if (
        bulk &&
        (bulkBytes + total > 22 * MAX_JSON_BYTES ||
          bulkCount >= 2 ||
          bulkOpenings.has(opening))
      )
        fail("busy");
      bytes += total;
      families.set(family, (families.get(family) || 0) + total);
      openings.set(opening, (openings.get(opening) || 0) + total);
      if (bulk) {
        bulkBytes += total;
        bulkCount++;
        bulkOpenings.add(opening);
      }
      let active = true;
      return () => {
        if (!active) return;
        active = false;
        bytes -= total;
        for (const [map, key] of [[families, family], [openings, opening]]) {
          const remaining = map.get(key) - total;
          if (remaining) map.set(key, remaining);
          else map.delete(key);
        }
        if (bulk) {
          bulkBytes -= total;
          bulkCount--;
          bulkOpenings.delete(opening);
        }
      };
    },
  };
}
