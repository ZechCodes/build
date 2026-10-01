// The notifications lab: the hero's field alone, full screen, on a clock
// the visitor controls. Bundled by Astro as an external module (the app's
// CSP drops inline script). Its state is in the query, so a link sent back
// opens on exactly what its sender was looking at.
import { advance } from "./clock.js";
import { mountControls } from "./controls.js";
import { createPlayer } from "./player.js";
import { readState, writeState } from "./state.js";
import { VARIANT_IDS } from "./variants.js";

// A gap longer than this (a tab coming back) moves the clock only this far.
// Anything shorter is real time: the compositor runs the animations on it,
// and a clock that fell behind would seek them back every frame.
const LONGEST_FRAME = 1;
// While it plays, the panel catches up this often, in ms, so it adds no
// paint of its own to every frame of the field.
const PANEL_EVERY = 250;
// A variant that runs wholly on the compositor (all but A's beat) is only
// seeked this often, in ms, not every frame: a script asking for every
// frame makes the browser restyle each of its hundreds of animated pills on
// every one.
const TICK = 100;

const container = document.querySelector("[data-lab-stage]");
const field = container.querySelector("[data-hero-field]");
const player = createPlayer({ container, field });
let state = readState(location.search, VARIANT_IDS);
// A variant's first frames style and paint every pill it moves, which can
// take longer than its whole field phase on a slow phone. The clock waits
// them out, so the beat is seen from its start.
const SETTLING_FRAMES = 2;
let settling = SETTLING_FRAMES;
function build() {
  built = player.build(state);
  settling = SETTLING_FRAMES;
}
let built;
build();
let show = () => {};

function remember() {
  const query = writeState(state);
  if (query !== location.search) history.replaceState(null, "", query);
}

const rebuilds = (patch) => ["variant", "endless"].some((key) => key in patch && patch[key] !== state[key]);

// What the controls do, each shown at once.
const act = {
  play() {
    state = { ...state, playing: !state.playing };
    render(performance.now());
  },
  restart() {
    state = { ...state, time: 0, playing: true };
    render(performance.now());
  },
  scrub(time) {
    state = { ...state, time, playing: false };
    render(performance.now());
  },
  set(patch) {
    const rebuild = rebuilds(patch);
    state = { ...state, ...patch, ...(rebuild ? { time: 0 } : {}) };
    if (rebuild) build();
    render(performance.now());
  },
};

let previous = null;
let shown = { at: -Infinity, query: "", time: null };
// Now when a setting changed or the held field was scrubbed; while it
// plays, every PANEL_EVERY ms.
function showNow(now) {
  const query = `${writeState(state)}|${built.end}`;
  const changed = query !== shown.query || (!state.playing && state.time !== shown.time);
  const due = state.playing && now - shown.at >= PANEL_EVERY;
  if (!changed && !due) return;
  shown = { at: now, query, time: state.time };
  show(state, built);
  remember();
}

function render(now) {
  const seconds = previous === null || settling > 0 ? 0 : Math.min(LONGEST_FRAME, (now - previous) / 1000);
  previous = now;
  state = advance(state, seconds, built.end);
  player.update(state.time, state.playing, state.speed);
  showNow(now);
}

// Every frame while a variant settles or needs the frames; otherwise a tick.
function tick(now) {
  render(now);
  if (settling > 0) settling -= 1;
  if (settling > 0 || built.everyFrame) requestAnimationFrame(tick);
  else setTimeout(() => tick(performance.now()), TICK);
}

// A new width lays the field out again: measured afresh, at the same moment.
let resizing = 0;
addEventListener("resize", () => {
  clearTimeout(resizing);
  resizing = setTimeout(build, 200);
});

addEventListener("keydown", (event) => {
  // A focused control answers its own keys.
  if (event.target !== document.body) return;
  if (event.key === " ") {
    event.preventDefault();
    act.play();
  } else if (event.key === "r") {
    act.restart();
  }
});

show = mountControls(document.querySelector("[data-lab-panel]"), act);
requestAnimationFrame(tick);

// For checks and web/hero-flood-probe.mjs, as window.BuildHero is.
window.BuildLab = {
  get state() { return state; },
  /** Whether the variant has had its first frames and the clock runs. */
  get settled() { return settling === 0; },
  get end() { return built.end; },
  get timing() { return built.timing; },
  set: act.set,
  scrub: act.scrub,
  restart: act.restart,
};
