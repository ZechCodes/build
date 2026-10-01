// Where the lab's ripple starts and its requests land. The lab has no
// laptop, so these are where the home page's laptop screen and its three
// Needs you rows stand at rest, as shares of the hero, measured in headless
// Chromium at 1440x900 and 390x844 (BuildHero.laptop.quads({ resting:
// true })). They change if the hero's layout does.
import { HERO_TIMING, NARROW_TIMING } from "../hero/timing.js";

export const NARROW_QUERY = "(max-width: 767px)";

const STAGES = Object.freeze({
  wide: Object.freeze({ origin: [0.724, 0.447], rows: [[0.748, 0.393], [0.748, 0.415], [0.748, 0.437]], nudge: 10, timing: HERO_TIMING }),
  narrow: Object.freeze({ origin: [0.558, 0.726], rows: [[0.611, 0.692], [0.611, 0.706], [0.611, 0.72]], nudge: 6, timing: NARROW_TIMING }),
});

export function stageFor(narrow, { width, height }) {
  const { origin, rows, nudge, timing } = narrow ? STAGES.narrow : STAGES.wide;
  const place = ([x, y]) => [x * width, y * height];
  return { origin: place(origin), rows: rows.map(place), nudge, timing };
}
