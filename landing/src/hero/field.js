// The hero's notification field, as data. Seeded, so every build draws the
// same field and nothing changes on a rerender; HeroField.astro renders it
// and entrance.js moves it. No DOM here.
//
// Each lane is two tracks meeting at its anchor (a fraction of the field's
// width): `before` runs left from the anchor, nearest first, and `after`
// runs right from it. The whole lane drifts right by `drift` vw over
// DRIFT_SECONDS, so the tracks carry enough pills to stay full while it
// does. An attention request is the first pill after its lane's anchor, so
// it starts exactly where it should be read.

// The harnesses Build runs today (bridge/src/harness, README "Supported
// agents"). Only these appear in the field; a harness from HARNESS_NAMES
// joins the list when the bridge supports it and its mark is in
// HeroField.astro.
export const SUPPORTED_HARNESSES = Object.freeze(["claude", "codex", "pi"]);
export const HARNESS_NAMES = Object.freeze({
  claude: "Claude Code",
  codex: "Codex",
  pi: "Pi",
  opencode: "OpenCode",
  gemini: "Gemini CLI",
  cursor: "Cursor",
});

// The three requests that need a person. `row` is the Needs you row each
// lands on (anchors.js); `lane` and `anchor` place it in the field, and
// `narrowAnchor` on a phone, where a pill is a larger share of the width.
export const ATTENTION = Object.freeze([
  Object.freeze({ id: "review", text: "Review ready", harness: "codex", row: "task-82", lane: 5, anchor: 0.5, narrowAnchor: 0.34 }),
  Object.freeze({ id: "approval", text: "Needs your approval", harness: "claude", row: "task-85", lane: 7, anchor: 0.58, narrowAnchor: 0.2 }),
  Object.freeze({ id: "question", text: "Which approach?", harness: "pi", row: "task-86", lane: 9, anchor: 0.4, narrowAnchor: 0.4 }),
]);

export const ROUTINE_EVENTS = Object.freeze([
  "Reading auth.ts", "Updating tests", "Tests passed", "Planning next step", "Checking dependencies",
  "Running build", "Changes ready", "Reading search.ts", "Editing archive.ts", "Running lint",
  "Lint clean", "Checking types", "Types pass", "Reading routes.ts", "Editing session.ts",
  "Writing migration", "Summarizing changes", "Indexing workspace", "Reading logs", "Updating docs",
  "Formatting files", "Resolving imports", "Running tests", "Build succeeded", "Committed 2 files",
  "Fetching origin", "Rebasing on main", "Reading package.json", "Updating lockfile", "Searching for usages",
  "Renaming handler", "Editing upload.ts", "Tests passed · 48", "Reading README.md", "Comparing branches",
  "Drafting plan", "Checking edge cases", "Reading config.ts", "Generating types", "Cleaning up",
]);

// Fast faint lanes behind, slower sharper ones in front. `drift` is vw over
// DRIFT_SECONDS; `gap` is the px between pills on a 1440px window, and
// scales with the window like the pills do (hero.css).
export const DRIFT_SECONDS = 6;
export const LANE_TIERS = Object.freeze({
  far: Object.freeze({ drift: [84, 120], gap: [64, 170] }),
  mid: Object.freeze({ drift: [42, 60], gap: [42, 132] }),
  near: Object.freeze({ drift: [18, 27], gap: [56, 170] }),
});

// The lanes top to bottom, and the ones a phone keeps.
const LANES = [
  ["far", false], ["mid", true], ["far", true], ["near", false],
  ["far", false], ["near", true], ["mid", true], ["near", true],
  ["far", true], ["near", true], ["mid", false], ["far", true],
];

// The narrowest pill with its gap is 7vw on a wide window.
const NARROWEST_PILL_VW = 7;

// mulberry32: small, fast and the same everywhere.
function random(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let value = state;
    value = Math.imul(value ^ (value >>> 15), value | 1);
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}

const between = (next, [low, high]) => low + (high - low) * next();
const round = (value, places = 3) => Number(value.toFixed(places));

function checkHarnesses(harnesses) {
  if (!harnesses.length) throw new Error("The field needs at least one harness.");
  for (const id of harnesses) {
    if (!HARNESS_NAMES[id]) throw new Error(`The field has no mark for harness ${id}.`);
  }
}

// A phrase unlike the one before it in its track, used least so far.
function pickEvent(next, used, previous) {
  const candidates = ROUTINE_EVENTS.filter((text) => text !== previous);
  const fewest = Math.min(...candidates.map((text) => used.get(text) || 0));
  const least = candidates.filter((text) => (used.get(text) || 0) === fewest);
  const text = least[Math.floor(next() * least.length)];
  used.set(text, fewest + 1);
  return text;
}

function track({ next, used, harnesses, tier, count, lead = null, key }) {
  const pills = lead ? [lead] : [];
  while (pills.length < count) {
    const text = pickEvent(next, used, pills.at(-1)?.text);
    const harness = harnesses[Math.floor(next() * harnesses.length)];
    pills.push({ key: `${key}-${pills.length}`, text, harness, gap: Math.round(between(next, LANE_TIERS[tier].gap)) });
  }
  return pills;
}

function lane(index, [tier, narrow], context) {
  const { next, harnesses } = context;
  const attention = ATTENTION.find((entry) => entry.lane === index);
  const drift = round(between(next, LANE_TIERS[tier].drift), 2);
  const anchor = attention ? attention.anchor : round(0.05 + next() * 0.9);
  const lead = attention
    ? { key: `attention-${attention.id}`, text: attention.text, harness: harnesses.includes(attention.harness) ? attention.harness : harnesses[0], gap: 0, attention: attention.id }
    : null;
  const shared = { ...context, tier };
  return {
    index,
    tier,
    narrow,
    drift,
    anchor,
    narrowAnchor: attention ? attention.narrowAnchor : anchor,
    // A lane's own small vertical offset, in px, so the rows are staggered.
    nudge: Math.round((next() - 0.5) * 14),
    before: track({ ...shared, key: `lane-${index}-before`, count: Math.ceil((anchor * 100 + drift) / NARROWEST_PILL_VW) }),
    after: track({ ...shared, key: `lane-${index}-after`, count: Math.ceil(((1 - anchor) * 100) / NARROWEST_PILL_VW) + 1, lead }),
  };
}

export function createField({ seed = 306, harnesses = SUPPORTED_HARNESSES } = {}) {
  checkHarnesses(harnesses);
  const context = { next: random(seed), used: new Map(), harnesses };
  let narrowRow = 0;
  const lanes = LANES.map((spec, index) => {
    const built = lane(index, spec, context);
    return spec[1] ? { ...built, narrowRow: narrowRow++ } : built;
  });
  return { lanes, narrowLanes: narrowRow };
}
