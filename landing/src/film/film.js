// The film: one pinned viewport, three devices and the close-ups that sit on
// their screens. Scroll drives one scrubbed timeline of low-information
// motion: device moves, and close-ups showing and going with their acts. As
// the playhead crosses an act's arrival, the act's copy comes in and its
// scene (what a person reads) plays on the clock, in seconds, so nobody has
// to know where to stop the wheel. The numbers live in acts.js.
import gsap from "gsap";
import { ScrollTrigger } from "gsap/ScrollTrigger";
import { createDeviceStage } from "../stage/stage.js";
import { STAGE_LIMITS, renderQualityScale } from "../stage/fallback.js";
import {
  ACTS,
  COPY_IN,
  COPY_OUT,
  POSES,
  SCENES,
  SCENE_SCREENS,
  SCREEN_CUES,
  TOTAL_TRAVEL,
  at,
  actAt,
  actStart,
  entrancePose,
  fullPose,
  sceneClock,
} from "./acts.js";
import { createScreenResolver, upcomingCues } from "./cues.js";
import { createGates } from "./gates.js";
import { createOverlays } from "./overlays.js";

gsap.registerPlugin(ScrollTrigger);

const SCRUB_SECONDS = 0.7;
const PRELOAD_LOOKAHEAD = 130;
const LID_ENTRANCE_START = 0.7;
const LID_ENTRANCE_SECONDS = 0.9;
const HANDOVER_SECONDS = 0.25;

function departPose(deviceName, settled) {
  return { ...entrancePose(deviceName, settled), x: settled.x + 14, y: settled.y + 6 };
}

function query(root, selector) {
  const element = root.querySelector(selector);
  if (!element) throw new Error(`The film needs ${selector}.`);
  return element;
}

// An act's copy comes in as the playhead enters the act and goes as it
// leaves; both on the clock, both undone by scrolling back over the gate.
function copyItems(act) {
  const copy = query(act, ".act__copy");
  return [...copy.children].filter((child) => !child.matches("[data-beat]"));
}

// One tween per act's copy at a time: the next move kills the last by its
// handle, since a staggered tween does not go when asked by target.
function showCopy(copy, instant, fromY = 28) {
  copy.tween?.kill();
  copy.tween = instant
    ? gsap.set(copy.items, { autoAlpha: 1, y: 0 })
    : gsap.fromTo(copy.items, { autoAlpha: 0, y: fromY }, { autoAlpha: 1, y: 0, duration: 0.7, stagger: 0.06, ease: "power2.out" });
}

function hideCopy(copy, instant, toY) {
  copy.tween?.kill();
  copy.tween = gsap.to(copy.items, { autoAlpha: 0, y: toY, duration: instant ? 0 : 0.35, ease: "power2.in" });
}

function copyGates(gates, acts) {
  for (const act of ACTS) {
    const copy = { items: copyItems(acts[act.id - 1]), tween: null };
    // The hero's copy is on the page from the first paint; the gate at 0
    // only has to take it back when the visitor returns.
    gates.add(at(act.id, act.id === 1 ? 0 : COPY_IN), (instant) => showCopy(copy, instant || act.id === 1), () => hideCopy(copy, false, 28));
    if (act.id !== 8) gates.add(at(act.id, COPY_OUT), (instant) => hideCopy(copy, instant, -18), () => showCopy(copy, false, -18));
  }
  beatGates(gates, acts[7]);
}

// Act 8's two beats over one pose: the first comes with the act, the second
// replaces it further in. One owner: each gate names which beat should be on
// screen and both beats go there from wherever they are, so a quick crossing
// or a jump cannot leave an earlier tween to finish on top of a later one.
function beatGates(gates, act) {
  const beats = { a: query(act, '[data-beat="a"]'), b: query(act, '[data-beat="b"]') };
  const tweens = new Map();
  const showBeat = (wanted, instant) => {
    for (const [name, element] of Object.entries(beats)) {
      tweens.get(name)?.kill();
      const on = name === wanted;
      const visible = gsap.getProperty(element, "autoAlpha") > 0.001;
      if (instant) {
        tweens.set(name, gsap.set(element, { autoAlpha: on ? 1 : 0, y: 0 }));
      } else if (on) {
        tweens.set(name, gsap.fromTo(element, { autoAlpha: visible ? gsap.getProperty(element, "autoAlpha") : 0, y: visible ? gsap.getProperty(element, "y") : 28 }, { autoAlpha: 1, y: 0, duration: 0.7, delay: wanted === "b" ? 0.3 : 0, ease: "power2.out" }));
      } else {
        tweens.set(name, gsap.to(element, { autoAlpha: 0, y: -18, duration: 0.35, ease: "power2.in" }));
      }
    }
  };
  gates.add(at(8, COPY_IN), (instant) => showBeat("a", instant), () => showBeat(null, false));
  gates.add(at(8, 0.6), (instant) => showBeat("b", instant), () => showBeat("a", false));
}

// A scene plays from its start each time its act arrives, and rewinds when
// the playhead leaves the act backwards, so scrolling back shows an act as
// it finished and scrolling on again shows it happen. A close-up that lifted
// stays up until the act's copy leaves; then the scene is brought to its end,
// if the visitor left early, and the departure plays. Crossing back plays the
// departure in reverse.
function sceneGates(gates, scenes, departures) {
  for (const [actId, scene] of Object.entries(scenes)) {
    const departure = departures[actId];
    gates.add(at(Number(actId), SCENES[actId].arrive), (instant) => {
      scene.pause();
      if (instant) {
        scene.time(scene.duration());
      } else {
        scene.time(0);
        scene.play();
      }
    }, () => {
      departure?.pause().progress(0);
      scene.pause();
      scene.time(0);
    });
    if (!departure) continue;
    gates.add(at(Number(actId), COPY_OUT), (instant) => {
      scene.pause();
      scene.progress(1);
      if (instant) departure.pause().progress(1);
      else departure.play(0);
    }, () => departure.reverse());
  }
}

function deviceTimeline(tl, pose) {
  const move = (device, to, from, until, ease = "power2.inOut") => {
    tl.to(pose[device], { ...fullPose(to), duration: until - from, ease }, from);
  };
  const laptop = POSES.laptop;
  move("laptop", laptop["1-typing"], at(1, 0.06), at(1, 0.2));
  move("laptop", laptop[2], at(1, 0.8), at(2, 0.22));
  move("laptop", laptop[3], at(2, 0.85), at(3, 0.2));
  move("laptop", laptop[4], at(3, 0.85), at(4, 0.2));
  move("laptop", laptop[5], at(4, 0.88), at(5, 0.15));
  move("laptop", laptop[6], at(5, 0.85), at(6, 0.12));
  move("laptop", laptop[7], at(6, 0.85), at(7, 0.05));
  move("laptop", laptop[8], at(7, 0.9), at(8, 0.35));

  const phone = POSES.phone;
  // In place as the act arrives, so its question is on screen before the
  // scene asks it.
  move("phone", phone[4], at(4, 0.04), at(4, 0.2), "power3.out");
  move("phone", departPose("phone", phone[4]), at(4, 0.85), at(4, 1), "power2.in");
  tl.set(pose.phone, fullPose(entrancePose("phone", phone[8])), at(8, 0));
  move("phone", phone[8], at(8, 0.05), at(8, 0.35), "power3.out");

  const tablet = POSES.tablet;
  move("tablet", tablet["7-arrive"], at(7, 0), at(7, 0.15), "power3.out");
  move("tablet", tablet[7], at(7, 0.15), at(7, 0.25));
  move("tablet", tablet[8], at(7, 0.9), at(8, 0.35));
}

function screenPreloader(stage) {
  const preloaded = new Set();
  const preload = (device, name) => {
    const key = `${device}:${name}`;
    if (preloaded.has(key)) return;
    preloaded.add(key);
    stage.preloadScreen(device, name).catch(() => preloaded.delete(key));
  };
  return (time) => {
    for (const [device, cues] of Object.entries(SCREEN_CUES)) {
      for (const name of upcomingCues(cues, time, PRELOAD_LOOKAHEAD)) preload(device, name);
    }
    for (const [actId, devices] of Object.entries(SCENE_SCREENS)) {
      const start = actStart(Number(actId));
      if (start > time + PRELOAD_LOOKAHEAD) continue;
      for (const [device, names] of Object.entries(devices)) names.forEach((name) => preload(device, name));
    }
  };
}

export function startFilm({ ignoreFrameBudget = false } = {}) {
  const root = document.documentElement;
  const film = query(document, "[data-film]");
  const canvas = query(film, "[data-stage]");
  const heroPoster = query(film, "[data-hero-poster]");
  const acts = [...film.querySelectorAll("[data-act]")];
  if (acts.length !== ACTS.length) throw new Error("The film needs eight acts.");
  acts.forEach((act, index) => { act.dataset.layout = ACTS[index].layout; });

  root.dataset.mode = "film";
  let stage;
  try {
    stage = createDeviceStage({ canvas });
  } catch (error) {
    delete root.dataset.mode;
    delete root.dataset.stage;
    throw error;
  }

  const pose = {
    laptop: fullPose(POSES.laptop[1]),
    phone: fullPose(entrancePose("phone", POSES.phone[4])),
    tablet: fullPose(entrancePose("tablet", POSES.tablet["7-arrive"])),
  };
  const overlays = createOverlays({ film, stage, pose });
  const sceneScreens = new Map();
  const resolveScreens = createScreenResolver(SCREEN_CUES, (device, name) => {
    stage.setScreen(device, name).catch(() => undefined);
  }, sceneScreens);
  const preload = screenPreloader(stage);
  const gates = createGates();

  const tl = gsap.timeline({
    defaults: { ease: "none" },
    scrollTrigger: {
      trigger: film,
      start: "top top",
      end: () => `+=${TOTAL_TRAVEL / 100 * innerHeight}`,
      pin: true,
      anticipatePin: 1,
      scrub: SCRUB_SECONDS,
      invalidateOnRefresh: true,
      // Raw scroll progress runs ahead of the smoothed playhead; it is
      // right for preloading and wrong for what is on a screen.
      onUpdate: (self) => {
        const time = self.progress * TOTAL_TRAVEL;
        preload(time);
        film.dataset.act = String(actAt(time).act);
      },
    },
  });
  deviceTimeline(tl, pose);
  overlays.addTo(tl);
  const scenes = overlays.scenes(sceneScreens);
  const departures = overlays.departures();
  copyGates(gates, acts);
  sceneGates(gates, scenes, departures);
  // The bar's hero-only variant leaves with the hero's copy. The persistent
  // one ignores the mark.
  const nav = document.querySelector(".site-nav");
  if (nav) {
    gates.add(at(1, COPY_OUT), () => { nav.dataset.pastHero = ""; }, () => { delete nav.dataset.pastHero; });
  }
  // The scrub maps scroll onto the timeline's whole duration; the last beat
  // does not run to the end of act 8, so hold the clock open to it.
  tl.set({}, {}, TOTAL_TRAVEL);

  let stopped = false;
  let firstFrameShown = false;
  // One rendered frame: the displays, the poses and the close-ups all read
  // the same clock, the timeline's, so a fast scroll cannot swap a screen
  // before the overlay that matches it arrives.
  const draw = () => {
    gates.update(tl.time());
    resolveScreens(tl.time());
    for (const [device, current] of Object.entries(pose)) stage.setPose(device, current);
    stage.render();
    overlays.update();
  };
  const frame = () => {
    if (stopped) return;
    const trigger = tl.scrollTrigger;
    const nearby = trigger && scrollY < trigger.end + innerHeight;
    if (!nearby) return;
    draw();
  };
  // The pin gives the film its box, and ScrollTrigger re-measures that box
  // after a resize on its own schedule; the canvas tells us when its box
  // actually changed, and that is when the stage's viewport must follow.
  const sync = () => {
    if (stopped) return;
    stage.resize();
    draw();
  };
  const boxWatcher = typeof ResizeObserver === "function" ? new ResizeObserver(sync) : null;
  boxWatcher?.observe(canvas);
  ScrollTrigger.addEventListener("refresh", sync);

  function stop(reason) {
    if (stopped) return;
    stopped = true;
    console.info(`The film stops (${reason}); the document stands.`);
    const { act } = actAt(tl.scrollTrigger.progress * TOTAL_TRAVEL);
    gsap.ticker.remove(frame);
    boxWatcher?.disconnect();
    ScrollTrigger.removeEventListener("refresh", sync);
    tl.scrollTrigger.kill();
    tl.kill();
    for (const scene of [...Object.values(scenes), ...Object.values(departures)]) scene.kill();
    gsap.set(acts.flatMap((act) => [...copyItems(act), ...act.querySelectorAll("[data-beat], .captions li")]), { clearProps: "all" });
    overlays.dispose();
    stage.dispose();
    delete root.dataset.mode;
    delete root.dataset.stage;
    delete film.dataset.act;
    if (nav) delete nav.dataset.pastHero;
    acts[act - 1].scrollIntoView({ block: "start", behavior: "instant" });
  }

  // The lid does the last quarter as a welcome: it plays once, on the first
  // real frame, only when the page opens at the top. Until that frame exists
  // the hero cutout holds its place: a capture of this stage at this pose,
  // lid where the welcome starts, so the hand-over is a crossfade between two
  // pictures of the same thing. A restored scroll position skips the welcome.
  function showFirstFrame() {
    if (firstFrameShown) return;
    firstFrameShown = true;
    const cost = stage.measureFrameCost();
    if (!ignoreFrameBudget && cost > STAGE_LIMITS.slowFrameMs) {
      stop(`frame budget: ${cost.toFixed(1)}ms`);
      return;
    }
    stage.setQualityScale(renderQualityScale(cost));
    // A page that opens mid-film has its copy and scenes where the gates put
    // them on the first frame; only the top gets the welcome. The copy is
    // already on the page and stays put.
    const opening = tl.scrollTrigger.progress * TOTAL_TRAVEL < at(1, 0.15);
    if (opening) {
      pose.laptop.lidOpen = LID_ENTRANCE_START;
      draw();
    }
    root.dataset.stage = "ready";
    const handover = gsap.timeline();
    handover.fromTo(canvas, { autoAlpha: 0 }, { autoAlpha: 1, duration: HANDOVER_SECONDS, ease: "none" }, 0);
    handover.to(heroPoster, { autoAlpha: 0, duration: HANDOVER_SECONDS, ease: "none" }, 0);
    if (opening) handover.to(pose.laptop, { lidOpen: 1, duration: LID_ENTRANCE_SECONDS, ease: "power3.out" }, HANDOVER_SECONDS);
  }

  // "See how it works" and "Join the waitlist" point at acts, in the film and
  // in the bar; inside the pin an anchor jump lands nowhere useful, so they
  // scroll the film instead. Act 8's form is on screen from its second beat.
  const goToAct = (actId, behavior) => {
    const local = actId === 8 ? 0.72 : 0.08;
    const trigger = tl.scrollTrigger;
    scrollTo({ top: trigger.start + (at(actId, local) / TOTAL_TRAVEL) * (trigger.end - trigger.start), behavior });
  };
  document.addEventListener("click", (event) => {
    const anchor = event.target.closest('a[href^="#act-"]');
    if (!anchor || stopped) return;
    const actId = Number(anchor.getAttribute("href").slice(5));
    if (!Number.isInteger(actId) || !ACTS.some((act) => act.id === actId)) return;
    event.preventDefault();
    goToAct(actId, "smooth");
  });
  // A click that came before this module did (the hero's call to action on
  // a slow connection), or a link to an act, left its act in the hash; the
  // film opens there rather than at the top, with that act's state set on
  // the first frame instead of played through from the hero.
  function openAtPendingAct() {
    const pendingAct = Number(/^#act-(\d+)$/.exec(location.hash)?.[1]);
    if (!ACTS.some((act) => act.id === pendingAct)) return;
    ScrollTrigger.refresh();
    goToAct(pendingAct, "instant");
    tl.scrollTrigger.update();
    tl.progress(tl.scrollTrigger.progress);
  }

  canvas.addEventListener("webglcontextlost", (event) => {
    event.preventDefault();
    stop("context lost");
  });
  matchMedia("(prefers-reduced-motion: reduce)").addEventListener("change", (event) => {
    if (event.matches) stop("reduced motion");
  });
  addEventListener("resize", () => {
    if (innerWidth < STAGE_LIMITS.documentMaxWidth) stop("viewport");
  });

  stage.load(["laptop"])
    .then(() => stage.setScreen("laptop", SCREEN_CUES.laptop[0][1]))
    .then(() => {
      openAtPendingAct();
      gates.update(tl.time());
      resolveScreens(tl.time());
      stage.resize();
      gsap.ticker.add(frame);
      showFirstFrame();
      return stage.load(["phone", "tablet"]);
    })
    .catch((error) => {
      console.warn(error);
      stop("load failed");
    });

  const film_ = {
    get progress() { return tl.scrollTrigger.progress; },
    seek(actId, local) {
      const trigger = tl.scrollTrigger;
      scrollTo({ top: trigger.start + (at(actId, local) / TOTAL_TRAVEL) * (trigger.end - trigger.start), behavior: "instant" });
    },
    time: () => tl.scrollTrigger.progress * TOTAL_TRAVEL,
    sync,
    timeline: tl,
    scenes,
    // Put an act's scene at a storyboard beat and hold it there, for a check
    // that wants a beat rather than the clock.
    sceneSeek(actId, local) {
      const scene = scenes[actId];
      if (!scene) return;
      scene.pause();
      scene.time(Math.min(scene.duration(), sceneClock(actId).at(actId, local)));
    },
    stage,
    pose,
    stop,
  };
  window.BuildFilm = film_;
  return film_;
}
