import { vi } from "vitest";

// A full-suite worker can spend more than vi.waitFor's default second on
// cache reads and DOM work. Keep a bounded assertion wait that returns as
// soon as the state is ready, including with a second full suite as load.
export const waitFor = (assertion, options = {}) => vi.waitFor(assertion, { timeout: 5_000, ...options });
