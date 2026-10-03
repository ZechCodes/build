// The hero lands on rows of a captured screen; the numbers it lands on must
// be the capture's, and the poster's must describe the poster.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { ATTENTION } from "../../src/hero/notifications.js";
import { HERO_ROW_REGIONS, HERO_SCREEN } from "../../src/hero/anchors.js";
import { HERO_POSTER } from "../../src/hero/poster-anchors.js";
import { SCREEN_CUES } from "../../src/film/acts.js";

const capture = JSON.parse(readFileSync(new URL("../../../design/landing-captures/capture-results.json", import.meta.url), "utf8"));

test("the row regions are the ones the capture recorded", () => {
  const record = capture.results.find((result) => `${result.state}-${result.profile}` === HERO_SCREEN);
  assert.ok(record, `capture-results.json has ${HERO_SCREEN}`);
  assert.deepEqual(
    Object.fromEntries(record.taskRows.map(({ task, rect }) => [task, rect])),
    Object.fromEntries(Object.entries(HERO_ROW_REGIONS).map(([task, rect]) => [task, [...rect]])),
  );
});

test("every attention request has a row on the screen and on the poster", () => {
  for (const entry of ATTENTION) {
    assert.ok(HERO_ROW_REGIONS[entry.row], entry.row);
    assert.equal(HERO_POSTER.rows[entry.row].length, 4, entry.row);
  }
});

test("the hero's laptop opens on that screen", () => {
  assert.equal(SCREEN_CUES.laptop[0][0], 0);
  assert.equal(SCREEN_CUES.laptop[0][1], HERO_SCREEN);
});

test("the poster's rows sit inside its screen, in order", () => {
  const [top, , bottom] = [HERO_POSTER.screen[0][1], 0, HERO_POSTER.screen[2][1]];
  let previous = -1;
  for (const entry of ATTENTION) {
    const quad = HERO_POSTER.rows[entry.row];
    for (const [x, y] of quad) assert.ok(x > 0 && x < 1 && y >= top && y <= bottom, entry.row);
    assert.ok(quad[0][1] > previous, `${entry.row} is below the row before`);
    previous = quad[0][1];
  }
});
