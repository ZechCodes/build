import { MAX_JSON_BYTES } from "./bridge-http-network.mjs";
import { fail } from "./bridge-http-contract.mjs";

// Reservations cover encoded request/response maxima, not JS object/decoded heap.
export function createWireBudget() {
  let bytes = 0;
  let bulkBytes = 0;
  let bulkCount = 0;
  const bulkOpenings = new Set();
  return {
    reserve({ requestMax = 0, responseMax = MAX_JSON_BYTES, opening } = {}) {
      const total = requestMax + responseMax;
      const bulk = requestMax > MAX_JSON_BYTES;
      if (bytes + total > 32 * MAX_JSON_BYTES) fail("busy");
      if (
        bulk &&
        (bulkBytes + total > 22 * MAX_JSON_BYTES ||
          bulkCount >= 2 ||
          bulkOpenings.has(opening))
      )
        fail("busy");
      bytes += total;
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
        if (bulk) {
          bulkBytes -= total;
          bulkCount--;
          bulkOpenings.delete(opening);
        }
      };
    },
  };
}
