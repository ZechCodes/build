// The film's mode is chosen before the first paint by a classic script in
// the head (a module runs after it). It must choose exactly as fallback.js
// does, and hand the page back to the document if the film never starts.
import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { stageMode } from "../../src/stage/fallback.js";

const source = readFileSync(new URL("../../public/film-boot.js", import.meta.url), "utf8");

function boot({ reducedMotion = false, viewportWidth = 1440, saveData = false, webgl = true, search = "" } = {}) {
  const dataset = {};
  const timers = [];
  const context = {
    document: {
      documentElement: { dataset },
      createElement: () => ({ getContext: (kind) => (webgl && kind === "webgl2" ? {} : null) }),
    },
    location: { search },
    innerWidth: viewportWidth,
    navigator: { connection: { saveData } },
    matchMedia: (query) => ({ matches: query.includes("reduce") && reducedMotion }),
    setTimeout: (callback, ms) => timers.push({ callback, ms }),
    URLSearchParams,
  };
  vm.runInNewContext(source, context);
  return { dataset, timers };
}

test("chooses the film exactly where fallback.js would", () => {
  const cases = [
    {},
    { reducedMotion: true },
    { viewportWidth: 767 },
    { viewportWidth: 768 },
    { viewportWidth: 390 },
    { saveData: true },
    { webgl: false },
  ];
  for (const signals of cases) {
    const expected = stageMode({ reducedMotion: false, viewportWidth: 1440, saveData: false, webgl: true, ...signals });
    const { dataset } = boot(signals);
    assert.equal(dataset.mode === "film" ? "stage" : "document", expected, JSON.stringify(signals));
  }
});

test("?film=0 keeps the document", () => {
  assert.equal(boot({ search: "?film=0" }).dataset.mode, undefined);
});

test("gives the page back to the document if the film never reports in", () => {
  const { dataset, timers } = boot();
  assert.equal(dataset.mode, "film");
  assert.equal(dataset.stage, "pending");
  assert.equal(timers.length, 1);
  timers[0].callback();
  assert.equal(dataset.mode, undefined);
  assert.equal(dataset.stage, undefined);
});

test("leaves a film that started alone", () => {
  for (const stage of ["starting", "ready"]) {
    const { dataset, timers } = boot();
    dataset.stage = stage;
    timers[0].callback();
    assert.equal(dataset.mode, "film", stage);
  }
});
