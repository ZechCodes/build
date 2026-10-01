// The hero's entrance: about four seconds, once per tab. The notification
// field (HeroField.astro) has been drifting on CSS since the first paint;
// this takes it over where it has got to and plays one GSAP timeline in the
// phases timing.js names: the field, a ripple from the laptop's screen that
// brakes and fades the routine pills, the laptop turning in, the three
// requests landing on their Needs you rows, the copy, and stillness. Then
// it takes the field out of the page and lets go of everything it touched.
//
// Scrolling, a change of width, reduced motion or a hidden-then-shown tab
// never wait on it: the first three finish it on the spot, the last pauses
// it. It never touches the scroll. A module later than the CSS that settles
// the hero without it leaves the hero at rest.
import gsap from "gsap";
import { ATTENTION, DRIFT_SECONDS } from "./field.js";
import { HERO_ROW_REGIONS, LANDING_POINT } from "./anchors.js";
import { createHeroLaptop } from "./laptop.js";
import { HERO_TIMING, NARROW_TIMING, pillPath, reachesField, revealEase, rippleReach } from "./timing.js";
import { homographyFromQuad, matrix3d, projectPoint } from "../stage/overlay.js";

export const PLAYED_KEY = "build.hero.played";
const NARROW_QUERY = "(max-width: 767px)";
// How far the wave pushes a pill out of its way, in px.
const NUDGE = { wide: 10, narrow: 6 };
// A landing request ends at this share of its size, as a row's height.
const LANDED_SCALE = 0.6;

function rememberPlayed() {
  try {
    sessionStorage.setItem(PLAYED_KEY, "1");
  } catch {
    // Storage refused: the next visit plays again, which is harmless.
  }
}

function query(root, selector) {
  const element = root.querySelector(selector);
  if (!element) throw new Error(`The hero needs ${selector}.`);
  return element;
}

const centreOf = (quad) => [quad.reduce((sum, [x]) => sum + x, 0) / quad.length, quad.reduce((sum, [, y]) => sum + y, 0) / quad.length];

// A point inside a row's quad, by its fractions across and down the row.
function pointOnRow(quad, [fx, fy]) {
  const homography = homographyFromQuad(1, 1, quad);
  return projectPoint(homography, fx, fy);
}

// The field where the CSS drift has got it: each lane's offset, how long it
// has been moving, and every pill's centre in the hero's pixels. The one
// read of layout the entrance makes; nothing reads it per frame.
function measureField(hero, field) {
  const heroBox = hero.getBoundingClientRect();
  const lanes = [...field.querySelectorAll(".hero-lane")].map((element) => {
    // A lane a phone leaves out has nothing to measure.
    if (!element.getClientRects().length) return { element, x: 0, speed: 0, pills: [], shown: false };
    const x = new DOMMatrixReadOnly(getComputedStyle(element).transform).m41;
    // The drift is in vw, and a vw is a hundredth of the window; a lane
    // running left has a negative one.
    const speed = (Number(element.dataset.drift) / 100) * innerWidth / DRIFT_SECONDS;
    const pills = [...element.querySelectorAll(".hero-pill")].map((pill) => {
      const box = pill.getBoundingClientRect();
      return {
        element: pill,
        attention: pill.dataset.attention || null,
        x: box.left - heroBox.left + box.width / 2,
        y: box.top - heroBox.top + box.height / 2,
        width: box.width,
        shown: box.width > 0,
        opacity: Number(getComputedStyle(pill).opacity),
      };
    });
    return { element, x, speed, pills, shown: pills.some((pill) => pill.shown) };
  });
  const moving = lanes.find((lane) => lane.shown && lane.speed !== 0);
  return { width: heroBox.width, height: heroBox.height, lanes, elapsed: moving ? moving.x / moving.speed : 0 };
}

// Where the timeline picks the field up: the time the CSS has already
// shown, but never so late that the field phase is gone.
const pickUpAt = (elapsed, timing) => Math.max(0, Math.min(elapsed, timing.field[1] - 0.4));

// The field moves on one clock, not a tween per pill: GSAP reads a pill's
// computed style when its tween starts, and hundreds of those reads, each
// after another tween's write, cost a phone whole frames. Everything here
// is worked out from the measured field (timing.js); each frame only
// writes, and only what changed.
function styleWriter() {
  const written = new WeakMap();
  return (element, transform, opacity = "") => {
    const last = written.get(element);
    if (last && last.transform === transform && last.opacity === opacity) return;
    written.set(element, { transform, opacity });
    element.style.transform = transform;
    element.style.opacity = opacity;
    element.style.visibility = opacity === "0" ? "hidden" : "";
  };
}

const px = (value) => `${Math.round(value * 10) / 10}px`;

// A lane drifts on until the ripple has passed, then holds: every pill it
// carries has faded or stopped by then.
const laneShift = (lane, time, { timing, start }) => lane.speed * (Math.min(time, timing.ripple[1]) - start);

// A pill nobody will see: hidden, gone past the far edge, or so far
// upstream that the lane stops before it could come in.
function inPlay(pill, lane, field, { timing, start }) {
  return pill.shown && reachesField(pill, lane.speed, field.width, timing.ripple[1] - start);
}

// Every pill that will be seen, with its way through the ripple.
function fieldPaths(field, ripple) {
  return field.lanes.flatMap((lane) => lane.pills
    .filter((pill) => inPlay(pill, lane, field, ripple))
    .map((pill) => ({ ...pill, lane, path: pillPath(pill, lane.speed, ripple) })));
}

// A routine pill's state at a moment, relative to its lane: untouched until
// the wave reaches it, then braking and fading.
function routineStyle(pill, time, ripple) {
  if (time <= pill.path.hit) return ["", ""];
  const { dx, dy, faded } = pill.path.at(time);
  const opacity = faded >= 1 ? "0" : String(Math.round(pill.opacity * (1 - faded) * 1000) / 1000);
  return [`translate(${px(dx - laneShift(pill.lane, time, ripple))}, ${px(dy)})`, opacity];
}

function requestStyle(pill, time, ripple) {
  if (time <= pill.path.hit) return [""];
  const { dx, dy } = pill.path.at(time);
  return [`translate(${px(dx - laneShift(pill.lane, time, ripple))}, ${px(dy)})`];
}

// The clock: lanes drift, routine pills brake and fade as the wave reaches
// them, and each request brakes and waits for its flight (`flights`, the
// time each takes off), which moves it from there.
function moveField(tl, { field, pills, ripple, flights, write }) {
  const until = Math.max(ripple.timing.ripple[1], ...flights.values());
  tl.to({}, {
    duration: until,
    ease: "none",
    onUpdate() {
      const time = this.time();
      for (const lane of field.lanes.filter((candidate) => candidate.shown)) write(lane.element, `translateX(${px(lane.x + laneShift(lane, time, ripple))})`);
      for (const pill of pills) {
        if (!pill.attention) write(pill.element, ...routineStyle(pill, time, ripple));
        else if (time < flights.get(pill.attention)) write(pill.element, ...requestStyle(pill, time, ripple));
      }
    },
  }, 0);
}

// A request flies from where it has got to to its row's landing point, read
// from where the laptop is at that moment, shrinking and fading as it
// arrives.
function landRequest(tl, pill, row, { laptop, timing, landing, ripple, write }) {
  const progress = { p: 0 };
  const takeOff = landing - timing.flight;
  const { dx, dy } = pill.path.at(takeOff);
  const base = [dx - laneShift(pill.lane, takeOff, ripple), dy];
  const from = [pill.x + dx, pill.y + dy];
  tl.fromTo(progress, { p: 0 }, {
    p: 1,
    duration: timing.flight,
    ease: "power2.inOut",
    immediateRender: false,
    onUpdate() {
      if (progress.p === 0) return;
      const [toX, toY] = pointOnRow(laptop.quads().rows[row], LANDING_POINT);
      const { p } = progress;
      const scale = 1 + (LANDED_SCALE - 1) * p;
      const opacity = p < 0.6 ? "" : String(Math.max(0, 1 - (p - 0.6) / 0.4));
      write(pill.element, `translate(${px(base[0] + (toX - from[0]) * p)}, ${px(base[1] + (toY - from[1]) * p)}) scale(${scale})`, opacity);
    },
  }, takeOff);
}

// The rows light as their requests land and settle back with the hero.
function glowRows(tl, glows, { laptop, timing }) {
  for (const glow of glows) {
    const [, , width, height] = HERO_ROW_REGIONS[glow.dataset.glow];
    glow.style.width = `${width}px`;
    glow.style.height = `${height}px`;
  }
  const place = () => {
    const { rows } = laptop.quads();
    for (const glow of glows) {
      const [, , width, height] = HERO_ROW_REGIONS[glow.dataset.glow];
      glow.style.transform = matrix3d(homographyFromQuad(width, height, rows[glow.dataset.glow]));
    }
  };
  tl.to({}, { duration: timing.settle[1] - timing.converge[0], onUpdate: place, onStart: place }, timing.converge[0]);
  ATTENTION.forEach((entry, index) => {
    const glow = glows.find((candidate) => candidate.dataset.glow === entry.row);
    tl.fromTo(glow, { opacity: 0 }, { opacity: 1, duration: 0.2, ease: "power2.out", immediateRender: false }, timing.landings[index] - 0.1);
  });
  tl.to(glows, { opacity: 0, duration: timing.settle[1] - timing.settle[0], ease: "power2.inOut" }, timing.settle[0]);
}

function revealCopy(tl, items, timing) {
  tl.fromTo(items, { autoAlpha: 0, y: 14 }, {
    autoAlpha: 1, y: 0, duration: 0.45, stagger: 0.07, ease: "power2.out", immediateRender: false,
  }, timing.message[0]);
}

function copyOf(hero) {
  const copy = query(hero, ".act__copy");
  const headline = query(copy, "h1");
  const lines = [...headline.querySelectorAll(".hero-line")];
  const rest = [...copy.children].filter((child) => child !== headline);
  return { headline, items: [...lines, ...rest], all: [headline, ...lines, ...rest] };
}

function buildTimeline({ hero, field, laptop, glows, copy, narrow }) {
  const timing = narrow ? NARROW_TIMING : HERO_TIMING;
  const measured = measureField(hero, field);
  const start = pickUpAt(measured.elapsed, timing);
  const origin = centreOf(laptop.quads({ resting: true }).screen);
  const ripple = {
    origin,
    reach: rippleReach(origin, measured),
    timing,
    start,
    nudge: narrow ? NUDGE.narrow : NUDGE.wide,
  };
  const tl = gsap.timeline({ paused: true });
  const write = styleWriter();
  const pills = fieldPaths(measured, ripple);
  const flights = new Map(ATTENTION.map((entry, index) => [entry.id, timing.landings[index] - timing.flight]));
  moveField(tl, { field: measured, pills, ripple, flights, write });
  tl.fromTo(laptop.reveal, { t: 0 }, {
    t: 1, duration: timing.laptop[1] - timing.laptop[0], ease: revealEase, onUpdate: laptop.render, immediateRender: false,
  }, timing.laptop[0]);
  ATTENTION.forEach((entry, index) => {
    const pill = pills.find((candidate) => candidate.attention === entry.id);
    if (pill) landRequest(tl, pill, entry.row, { laptop, timing, landing: timing.landings[index], ripple, write });
  });
  glowRows(tl, glows, { laptop, timing });
  revealCopy(tl, copy.items, timing);
  return { tl, timing, start, lanes: measured.lanes };
}

// Hold the copy and the laptop out of sight before the CSS that has been
// holding them lets go: html[data-hero="playing"].
function takeOver(root, { copy, laptop }) {
  gsap.set(copy.items, { autoAlpha: 0 });
  gsap.set(copy.headline, { autoAlpha: 1 });
  laptop.render();
  root.dataset.hero = "playing";
}

function listen(finish, timeline) {
  const width = innerWidth;
  const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
  const onScroll = () => { if (scrollY > 8) finish("scroll"); };
  const onResize = () => { if (Math.abs(innerWidth - width) > 1) finish("resize"); };
  const onMotion = (event) => { if (event.matches) finish("reduced motion"); };
  const onVisibility = () => { if (document.hidden) timeline.pause(); else timeline.play(); };
  addEventListener("scroll", onScroll, { passive: true });
  addEventListener("resize", onResize);
  reducedMotion.addEventListener("change", onMotion);
  document.addEventListener("visibilitychange", onVisibility);
  return () => {
    removeEventListener("scroll", onScroll);
    removeEventListener("resize", onResize);
    reducedMotion.removeEventListener("change", onMotion);
    document.removeEventListener("visibilitychange", onVisibility);
  };
}

// Whether the CSS that settles the hero without this module (hero.css,
// hero-fallback-out) has begun to take the field away. Its clock is the time
// since the first paint; played from there, the entrance would hide copy the
// visitor may already be reading. A finished fallback has taken the field
// out of rendering, and its animation with it.
function fallbackBegun(field) {
  const fallback = field.getAnimations?.().find((animation) => animation.animationName === "hero-fallback-out");
  if (!fallback) return getComputedStyle(field).display === "none";
  return fallback.currentTime >= fallback.effect.getTiming().delay;
}

// Not playing: the hero at rest, the field gone.
function rest(root, field) {
  field?.remove();
  delete root.dataset.hero;
  return null;
}

/** The hero at rest after an entrance that could not play: nothing it may
 *  have set stays on the page. */
export function restHero(root = document.documentElement) {
  const hero = document.querySelector("#act-1");
  if (hero) gsap.set(hero.querySelectorAll(".act__copy, .act__copy *, [data-hero-device], [data-hero-frame], [data-glow]"), { clearProps: "all" });
  return rest(root, hero?.querySelector("[data-hero-field]"));
}

/** Start the entrance if hero-boot.js chose it. Returns its handle, or null
 *  when the hero is simply at rest. */
export function startHeroEntrance({ root = document.documentElement } = {}) {
  if (root.dataset.hero !== "entrance") return rest(root, document.querySelector("[data-hero-field]"));
  const hero = query(document, "#act-1");
  const field = hero.querySelector("[data-hero-field]");
  if (!field || scrollY > 8 || matchMedia("(prefers-reduced-motion: reduce)").matches || fallbackBegun(field)) return rest(root, field);
  rememberPlayed();
  const narrow = matchMedia(NARROW_QUERY).matches;
  const device = query(hero, "[data-hero-device]");
  const laptop = createHeroLaptop({ hero, device, frame: query(device, "[data-hero-frame]"), narrow });
  const glows = [...hero.querySelectorAll("[data-glow]")];
  const copy = copyOf(hero);
  const built = buildTimeline({ hero, field, laptop, glows, copy, narrow });
  const { tl, timing } = built;
  const settled = [];
  let done = false;
  let held = false;
  let unlisten = () => {};

  function finish(reason = "done") {
    if (done) return;
    done = true;
    unlisten();
    tl.progress(1);
    tl.kill();
    laptop.settle();
    field.remove();
    gsap.set(glows, { clearProps: "all" });
    gsap.set(copy.all, { clearProps: "opacity,visibility,transform" });
    delete root.dataset.hero;
    entrance.reason = reason;
    for (const callback of settled.splice(0)) callback();
  }

  tl.call(() => { if (!held) finish(); }, null, timing.settle[1]);
  takeOver(root, { copy, laptop });
  unlisten = listen(finish, tl);
  tl.time(built.start).play();

  const entrance = {
    timeline: tl,
    timing,
    laptop,
    reason: null,
    get done() { return done; },
    finish,
    /** For the development scrubber: stop at the end instead of letting go. */
    hold() { held = true; },
    attachStage: (api) => (done ? "handover" : laptop.attachStage(api)),
    whenSettled(callback) {
      if (done) callback();
      else settled.push(callback);
    },
  };
  return entrance;
}
