// The localStorage every suite runs with, on whichever Node the tree is run on.
//
// Node 25 shipped an experimental Web Storage implementation. It installs
// `localStorage` on globalThis as an accessor pair, and that accessor answers
// `undefined` unless the process was started with `--localstorage-file`.
// vitest's jsdom environment leaves a global that is already defined alone, so
// jsdom's own Storage never lands and `localStorage.getItem` throws on the
// first line src/app.js reads a preference from. A node-environment suite has
// never had a Storage of its own at all.
//
// So one is installed here for both, and with Object.defineProperty: a plain
// assignment lands on Node's setter and changes nothing. In vitest's jsdom
// environment `window` IS globalThis, so this is `window.localStorage` there
// and a bare global in the node one — one storage, one definition.
//
// Held in memory and minted per file, which is the isolation jsdom's own
// Storage gave: no suite reads what another one wrote.

/** A Storage, as much of one as this client asks for: text in, text out, and
 *  the length/key pair that makes it enumerable. */
function memoryStorage() {
  const held = new Map();
  return {
    get length() {
      return held.size;
    },
    key: (index) => [...held.keys()][index] ?? null,
    getItem: (name) => (held.has(String(name)) ? held.get(String(name)) : null),
    setItem: (name, value) => void held.set(String(name), String(value)),
    removeItem: (name) => void held.delete(String(name)),
    clear: () => held.clear(),
  };
}

if (typeof globalThis.localStorage === "undefined") {
  Object.defineProperty(globalThis, "localStorage", {
    value: memoryStorage(),
    configurable: true,
    writable: true,
  });
}
