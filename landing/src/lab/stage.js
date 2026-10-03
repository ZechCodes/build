// Where the lab's ripple starts and its requests land. The lab has no
// laptop, so these are where the home page's laptop screen and its three
// Needs you rows stand at rest, as shares of the hero, measured in headless
// Chromium at 1440x900 and 390x844 (BuildHero.laptop.quads({ resting:
// true })). `headline` and `screen` are the boxes of the hero's h1 and the
// laptop's screen there, [left, top, right, bottom]; `clear` the higher of
// their tops, above which the wall's dome stays (#316). They change if the
// hero's layout does.
import { HERO_TIMING, NARROW_TIMING } from "./legacy/timing.js";

export const NARROW_QUERY = "(max-width: 767px)";

const STAGES = Object.freeze({
  wide: Object.freeze({
    origin: [0.724, 0.447], rows: [[0.748, 0.393], [0.748, 0.415], [0.748, 0.437]], nudge: 10, timing: HERO_TIMING,
    headline: [0.089, 0.221, 0.444, 0.517], screen: [0.555, 0.248, 0.89, 0.59],
  }),
  narrow: Object.freeze({
    origin: [0.558, 0.726], rows: [[0.611, 0.692], [0.611, 0.706], [0.611, 0.72]], nudge: 6, timing: NARROW_TIMING,
    headline: [0.051, 0.15, 0.949, 0.274], screen: [0.187, 0.57, 0.921, 0.787],
  }),
});

export function stageFor(narrow, { width, height }) {
  const { origin, rows, nudge, timing, headline, screen } = narrow ? STAGES.narrow : STAGES.wide;
  const place = ([x, y]) => [x * width, y * height];
  const box = ([left, top, right, bottom]) => [...place([left, top]), ...place([right, bottom])];
  return { origin: place(origin), rows: rows.map(place), nudge, timing, headline: box(headline), screen: box(screen), clear: Math.min(headline[1], screen[1]) * height };
}
