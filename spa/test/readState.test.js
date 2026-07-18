import { describe, it, expect } from "vitest";
import { READ_IDS_KEY, loadReadIds, persistReadIds, pruneReadIds } from "../src/core/readState.js";

// A minimal localStorage stand-in so the module stays testable under node.
function fakeStorage(initial = {}) {
  const store = new Map(Object.entries(initial));
  return {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    _store: store,
  };
}

describe("loadReadIds / persistReadIds", () => {
  it("round-trips a set of ids through storage", () => {
    const storage = fakeStorage();
    persistReadIds(new Set(["run-1", "plan-2"]), storage);
    const loaded = loadReadIds(storage);
    expect(loaded).toBeInstanceOf(Set);
    expect([...loaded].sort()).toEqual(["plan-2", "run-1"]);
  });

  it("returns an empty set when the key is missing", () => {
    expect(loadReadIds(fakeStorage()).size).toBe(0);
  });

  it("returns an empty set on corrupt JSON without throwing", () => {
    const storage = fakeStorage({ [READ_IDS_KEY]: "{not json[" });
    expect(loadReadIds(storage).size).toBe(0);
  });

  it("returns an empty set when the stored JSON is not an array", () => {
    const storage = fakeStorage({ [READ_IDS_KEY]: '{"a":1}' });
    expect(loadReadIds(storage).size).toBe(0);
  });

  it("persists as a JSON array under the versioned key", () => {
    const storage = fakeStorage();
    persistReadIds(new Set(["run-1"]), storage);
    expect(JSON.parse(storage._store.get(READ_IDS_KEY))).toEqual(["run-1"]);
  });
});

describe("pruneReadIds", () => {
  it("drops ids that vanished from the feed and keeps live ones", () => {
    const pruned = pruneReadIds(new Set(["run-1", "run-gone", "plan-1"]), new Set(["run-1", "plan-1", "plan-new"]));
    expect([...pruned].sort()).toEqual(["plan-1", "run-1"]);
  });

  it("accepts liveIds as an array", () => {
    const pruned = pruneReadIds(new Set(["a", "b"]), ["b", "c"]);
    expect([...pruned]).toEqual(["b"]);
  });

  it("is pure — the input set is not mutated and a new set is returned", () => {
    const input = new Set(["a", "gone"]);
    const pruned = pruneReadIds(input, ["a"]);
    expect(pruned).not.toBe(input);
    expect([...input].sort()).toEqual(["a", "gone"]);
  });
});
