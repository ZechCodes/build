import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../../public/hero333-boot.js", import.meta.url), "utf8");

function boot({ reducedMotion = false, search = "", hash = "" } = {}) {
  const dataset = {};
  const storage = new Map([["build.hero.played", "1"]]);
  let writes = 0;
  const sessionStorage = {
    getItem: (key) => storage.get(key) ?? null,
    setItem: (key, value) => { writes += 1; storage.set(key, value); },
    removeItem: (key) => { writes += 1; storage.delete(key); },
  };
  vm.runInNewContext(source, {
    document: { documentElement: { dataset } },
    location: { search, hash },
    matchMedia: (query) => ({ matches: query.includes("reduce") && reducedMotion }),
    sessionStorage,
    URLSearchParams,
  });
  return { hero: dataset.hero, played: storage.get("build.hero.played"), writes };
}

test("the preview replays despite an earlier main-site visit without changing its marker", () => {
  assert.deepEqual(boot(), { hero: "entrance", played: "1", writes: 0 });
  assert.deepEqual(boot({ search: "?hero=play" }), { hero: "entrance", played: "1", writes: 0 });
});

test("reduced motion and hero=0 keep the preview at rest", () => {
  assert.deepEqual(boot({ reducedMotion: true }), { hero: undefined, played: "1", writes: 0 });
  assert.deepEqual(boot({ reducedMotion: true, search: "?hero=play" }), { hero: undefined, played: "1", writes: 0 });
  assert.deepEqual(boot({ search: "?hero=0" }), { hero: undefined, played: "1", writes: 0 });
});
