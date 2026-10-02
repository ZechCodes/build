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
import { ATTENTION } from "./field.js";
import { HERO_ROW_REGIONS, LANDING_POINT } from "./anchors.js";
import { createHeroLaptop } from "./laptop.js";
import { createFieldMotion } from "./flood.js";
import { measureField } from "./measure.js";
import { HERO_TIMING, NARROW_TIMING, SHORT_NARROW_TIMING, requestFlight, revealEase, rippleReach } from "./timing.js";
import { homographyFromQuad, matrix3d, projectPoint } from "../../stage/overlay.js";

export const PLAYED_KEY = "build.hero.played";
const NARROW_QUERY = "(max-width: 767px)";

function rememberPlayed() {
  // The preview never changes the home page's per-tab entrance memory.
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

// Where the timeline picks the field up: the time the CSS has already
// shown, but never so late that the field phase is gone.
const pickUpAt = (elapsed, timing) => Math.max(0, Math.min(elapsed, timing.field[1] - 0.4));

// The field on the entrance's clock: flood.js moves it on the compositor,
// seeked to this tween, which runs until the last request takes off.
function moveField(tl, motion, { timing, flights }) {
  const until = Math.max(timing.ripple[1], ...flights.values());
  tl.to({}, {
    duration: until,
    ease: "none",
    onUpdate() {
      motion.update(this.time(), !tl.paused());
    },
  }, 0);
}

// A request flies from where it stopped to its row's landing point, read
// from where the laptop is at that moment, shrinking and fading as it
// arrives.
function landRequest(tl, pill, row, { laptop, timing, landing, motion }) {
  const progress = { p: 0 };
  const takeOff = landing - timing.flight;
  const base = motion.offsetAt(pill, takeOff);
  const { dx, dy } = motion.pathOf(pill).at(takeOff);
  const from = [pill.x + dx, pill.y + dy];
  tl.fromTo(progress, { p: 0 }, {
    p: 1,
    duration: timing.flight,
    ease: "power2.inOut",
    immediateRender: false,
    onUpdate() {
      if (progress.p === 0) return;
      const to = pointOnRow(laptop.quads().rows[row], LANDING_POINT);
      const { transform, opacity } = requestFlight(progress.p, { base, from, to });
      pill.element.style.transform = transform;
      pill.element.style.opacity = String(opacity);
      pill.element.style.visibility = opacity === 0 ? "hidden" : "";
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
  const timing = narrow ? (matchMedia("(max-height: 760px)").matches ? SHORT_NARROW_TIMING : NARROW_TIMING) : HERO_TIMING;
  const measured = measureField(hero, field);
  const start = pickUpAt(measured.elapsed, timing);
  const origin = centreOf(laptop.quads({ resting: true }).screen);
  const ripple = {
    origin,
    reach: rippleReach(origin, measured),
    timing,
    start,
  };
  const tl = gsap.timeline({ paused: true });
  const flights = new Map(ATTENTION.map((entry, index) => [entry.id, timing.landings[index] - timing.flight]));
  const motion = createFieldMotion({ field: measured, ripple, flights });
  moveField(tl, motion, { timing, flights });
  tl.fromTo(laptop.reveal, { t: 0 }, {
    t: 1, duration: timing.laptop[1] - timing.laptop[0], ease: revealEase, onUpdate: laptop.render, immediateRender: false,
  }, timing.laptop[0]);
  const requests = measured.lanes.flatMap((lane) => lane.pills.filter((pill) => pill.attention && motion.pathOf(pill)));
  ATTENTION.forEach((entry, index) => {
    const pill = requests.find((candidate) => candidate.attention === entry.id);
    if (pill) landRequest(tl, pill, entry.row, { laptop, timing, landing: timing.landings[index], motion });
  });
  glowRows(tl, glows, { laptop, timing });
  revealCopy(tl, copy.items, timing);
  return { tl, timing, start, lanes: measured.lanes, motion };
}

// Hold the copy and the laptop out of sight before the CSS that has been
// holding them lets go: html[data-hero="playing"].
function takeOver(root, { copy, laptop }) {
  gsap.set(copy.items, { autoAlpha: 0 });
  gsap.set(copy.headline, { autoAlpha: 1 });
  laptop.render();
  root.dataset.hero = "playing";
}

function listen(finish, timeline, motion) {
  const width = innerWidth;
  const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
  const onScroll = () => { if (scrollY > 8) finish("scroll"); };
  const onResize = () => { if (Math.abs(innerWidth - width) > 1) finish("resize"); };
  const onMotion = (event) => { if (event.matches) finish("reduced motion"); };
  // The field's animations run on the compositor's time, not the timeline's:
  // held with it, so a returning tab picks up where it left.
  const onVisibility = () => {
    if (!document.hidden) return timeline.play();
    timeline.pause();
    motion.update(timeline.time(), false);
  };
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
  unlisten = listen(finish, tl, built.motion);
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
