// The notifications lab: the hero's field alone, full screen, on a clock
// the visitor controls. Bundled by Astro as an external module (the app's
// CSP drops inline script). Its state is in the query, so a link sent back
// opens on exactly what its sender was looking at.
import { advance } from "./clock.js";
import { mountControls } from "./controls.js";
import { createPlayer } from "./player.js";
import { readState, writeState } from "./state.js";
import { VARIANT_IDS } from "./variants.js";

// A frame longer than this (a tab coming back) moves the clock only this far.
const LONGEST_FRAME = 0.1;

const container = document.querySelector("[data-lab-stage]");
const field = container.querySelector("[data-hero-field]");
const player = createPlayer({ container, field });
let state = readState(location.search, VARIANT_IDS);
let built = player.build(state);
let show = () => {};

function remember() {
  const query = writeState(state);
  if (query !== location.search) history.replaceState(null, "", query);
}

const rebuilds = (patch) => ["variant", "endless"].some((key) => key in patch && patch[key] !== state[key]);

const act = {
  play() {
    state = { ...state, playing: !state.playing };
  },
  restart() {
    state = { ...state, time: 0, playing: true };
  },
  scrub(time) {
    state = { ...state, time, playing: false };
  },
  set(patch) {
    const rebuild = rebuilds(patch);
    state = { ...state, ...patch, ...(rebuild ? { time: 0 } : {}) };
    if (rebuild) built = player.build(state);
  },
};

let previous = null;
function frame(now) {
  const seconds = previous === null ? 0 : Math.min(LONGEST_FRAME, (now - previous) / 1000);
  previous = now;
  state = advance(state, seconds, built.end);
  player.update(state.time, state.playing, state.speed);
  show(state, built);
  remember();
  requestAnimationFrame(frame);
}

// A new width lays the field out again: measured afresh, at the same moment.
let resizing = 0;
addEventListener("resize", () => {
  clearTimeout(resizing);
  resizing = setTimeout(() => { built = player.build(state); }, 200);
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
requestAnimationFrame(frame);

// For checks and web/hero-flood-probe.mjs, as window.BuildHero is.
window.BuildLab = {
  get state() { return state; },
  get end() { return built.end; },
  get timing() { return built.timing; },
  set: act.set,
  scrub: act.scrub,
  restart: act.restart,
};
