// hero-boot.js decides before the first paint whether the entrance plays.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../../public/hero-boot.js", import.meta.url), "utf8");

function boot({ reducedMotion = false, search = "", hash = "", played = false, storage = "ok" } = {}) {
  const dataset = {};
  const sessionStorage = {
    getItem(key) {
      if (storage === "throws") throw new Error("SecurityError");
      return played && key === "build.hero.played" ? "1" : null;
    },
  };
  vm.runInNewContext(source, {
    document: { documentElement: { dataset } },
    location: { search, hash },
    matchMedia: (query) => ({ matches: query.includes("reduce") && reducedMotion }),
    sessionStorage,
    URLSearchParams,
  });
  return dataset.hero;
}

test("a first visit plays the entrance", () => {
  assert.equal(boot(), "entrance");
});

test("a second visit in the same tab shows the hero at rest", () => {
  assert.equal(boot({ played: true }), undefined);
});

test("storage that refuses plays the entrance rather than failing", () => {
  assert.equal(boot({ storage: "throws" }), "entrance");
});

test("reduced motion shows the hero at rest, even when forced", () => {
  assert.equal(boot({ reducedMotion: true }), undefined);
  assert.equal(boot({ reducedMotion: true, search: "?hero=play" }), undefined);
});

test("a link to a later act skips the entrance; the hero's own does not", () => {
  assert.equal(boot({ hash: "#act-4" }), undefined);
  assert.equal(boot({ hash: "#act-1" }), "entrance");
  assert.equal(boot({ hash: "#details" }), "entrance");
});

test("?hero=play and ?hero=0 force either way", () => {
  assert.equal(boot({ played: true, search: "?hero=play" }), "entrance");
  assert.equal(boot({ search: "?hero=0" }), undefined);
});
