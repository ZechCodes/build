// Shared notification copy, supported harnesses, and seeded selection for the home wall and legacy lab.
// The harnesses Build runs today (bridge/src/harness, README "Supported
// agents"). Only these appear in the field; a harness from HARNESS_NAMES
// joins the list when the bridge supports it and its mark is in
// HeroField.astro and hero.css.
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
// lands on (anchors.js). The legacy lab owns its separate lane placement.
export const ATTENTION = Object.freeze([
  Object.freeze({ id: "review", text: "Review ready", harness: "codex", row: "task-82" }),
  Object.freeze({ id: "approval", text: "Needs your approval", harness: "claude", row: "task-85" }),
  Object.freeze({ id: "question", text: "Which approach?", harness: "pi", row: "task-86" }),
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

// mulberry32: small, fast and the same everywhere. The lab's variants draw
// from it too.
export function random(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let value = state;
    value = Math.imul(value ^ (value >>> 15), value | 1);
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}
