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
// it. It never touches the scroll.
import gsap from "gsap";
import { ATTENTION, DRIFT_SECONDS } from "./field.js";
import { HERO_ROW_REGIONS, LANDING_POINT } from "./anchors.js";
import { createHeroLaptop } from "./laptop.js";
import { HERO_TIMING, NARROW_TIMING, decelDistance, outward, revealEase, rippleHit, rippleReach } from "./timing.js";
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
    const x = new DOMMatrixReadOnly(getComputedStyle(element).transform).m41;
    // The drift is in vw, and a vw is a hundredth of the window.
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
  const moving = lanes.find((lane) => lane.shown && lane.speed > 0);
  return { width: heroBox.width, height: heroBox.height, lanes, elapsed: moving ? moving.x / moving.speed : 0 };
}

// Where the timeline picks the field up: the time the CSS has already
// shown, but never so late that the field phase is gone.
const pickUpAt = (elapsed, timing) => Math.max(0, Math.min(elapsed, timing.field[1] - 0.4));

function driftLanes(tl, lanes, start, timing) {
  const until = timing.ripple[0];
  for (const lane of lanes) {
    const from = lane.x - lane.speed * start;
    tl.fromTo(lane.element, { x: from }, { x: from + lane.speed * until, duration: until, ease: "none", immediateRender: false }, 0);
  }
}

// One pill as the wave reaches it: it carries on at its lane's speed until
// then, brakes to a stop pushed a little outward, and, unless it needs a
// person, fades as it brakes. Returns where it stops.
function ripplePill(tl, pill, lane, ripple) {
  const { origin, reach, timing, start, nudge } = ripple;
  const begin = timing.ripple[0];
  const x = pill.x + lane.speed * (begin - start);
  const hit = rippleHit([x, pill.y], origin, reach, timing);
  const travelled = lane.speed * (hit - begin);
  const [dx, dy] = outward([x, pill.y], origin, nudge);
  const stopX = travelled + decelDistance(lane.speed, timing.decel) + dx;
  if (hit > begin) tl.fromTo(pill.element, { x: 0 }, { x: travelled, duration: hit - begin, ease: "none", immediateRender: false }, begin);
  tl.fromTo(pill.element, { x: travelled, y: 0 }, { x: stopX, y: dy, duration: timing.decel, ease: "power1.out", immediateRender: false }, hit);
  if (!pill.attention) {
    tl.fromTo(pill.element, { autoAlpha: pill.opacity }, { autoAlpha: 0, duration: timing.fade, ease: "power1.in", immediateRender: false }, hit);
  }
  return { offset: [stopX, dy], at: [x + stopX, pill.y + dy] };
}

// A pill nobody will see: past the right edge, or so far left that the wave
// fades it before it could come in.
function offField(pill, lane, field, timing) {
  const reachIn = lane.speed * (timing.ripple[1] - timing.ripple[0]);
  return !pill.shown || pill.x - pill.width / 2 > field.width || pill.x + pill.width / 2 < -reachIn;
}

function rippleField(tl, field, ripple) {
  const stops = new Map();
  for (const lane of field.lanes) {
    for (const pill of lane.pills) {
      if (offField(pill, lane, field, ripple.timing)) {
        tl.set(pill.element, { autoAlpha: 0 }, ripple.timing.ripple[0]);
        continue;
      }
      const stop = ripplePill(tl, pill, lane, ripple);
      if (pill.attention) stops.set(pill.attention, { ...stop, element: pill.element });
    }
  }
  return stops;
}

// A request flies from where it stopped to its row's landing point, read
// from where the laptop is at that moment, shrinking and fading as it
// arrives.
function landRequest(tl, stop, row, { laptop, timing, landing }) {
  const progress = { p: 0 };
  const [baseX, baseY] = stop.offset;
  const [fromX, fromY] = stop.at;
  tl.fromTo(progress, { p: 0 }, {
    p: 1,
    duration: timing.flight,
    ease: "power2.inOut",
    immediateRender: false,
    onUpdate() {
      if (progress.p === 0) return;
      const [toX, toY] = pointOnRow(laptop.quads().rows[row], LANDING_POINT);
      const { p } = progress;
      gsap.set(stop.element, {
        x: baseX + (toX - fromX) * p,
        y: baseY + (toY - fromY) * p,
        scale: 1 + (LANDED_SCALE - 1) * p,
        autoAlpha: p < 0.6 ? 1 : 1 - (p - 0.6) / 0.4,
      });
    },
  }, landing - timing.flight);
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
  driftLanes(tl, measured.lanes, start, timing);
  const stops = rippleField(tl, measured, ripple);
  tl.fromTo(laptop.reveal, { t: 0 }, {
    t: 1, duration: timing.laptop[1] - timing.laptop[0], ease: revealEase, onUpdate: laptop.render, immediateRender: false,
  }, timing.laptop[0]);
  ATTENTION.forEach((entry, index) => {
    const stop = stops.get(entry.id);
    if (stop) landRequest(tl, stop, entry.row, { laptop, timing, landing: timing.landings[index] });
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
  if (root.dataset.hero !== "entrance") return null;
  const hero = query(document, "#act-1");
  const field = hero.querySelector("[data-hero-field]");
  if (!field || scrollY > 8 || matchMedia("(prefers-reduced-motion: reduce)").matches) return rest(root, field);
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
