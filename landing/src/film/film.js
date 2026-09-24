// The film: one pinned viewport, three devices and the close-ups that sit on
// their screens. Scroll only moves the devices from one act's resting point
// to the next, the same distance every time, and eases onto the nearest
// resting point when the visitor stops. Arriving there is the one cue: the
// act's copy comes in, and with it the act's beat plays on the clock for
// BEAT_SECONDS and ends still, which says "scroll on". The scroll is always
// the visitor's; an act left mid-beat is finished when they come back to it.
// The numbers live in acts.js.
import gsap from "gsap";
import { ScrollTrigger } from "gsap/ScrollTrigger";
import { createDeviceStage } from "../stage/stage.js";
import { STAGE_LIMITS, renderQualityScale } from "../stage/fallback.js";
import {
  ACTS,
  BEAT_EASE,
  BEAT_SECONDS,
  POSES,
  SCENE_SCREENS,
  SCREEN_CUES,
  TOTAL_TRAVEL,
  at,
  actAt,
  actStart,
  arrivalAt,
  between,
  entrancePose,
  fullPose,
  restAt,
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

// An act named in the hash (#act-8): a link, or a call to action pressed
// before the film was running to take it over.
function hashedAct() {
  const actId = Number(/^#act-(\d+)$/.exec(location.hash)?.[1]);
  return ACTS.some((act) => act.id === actId) ? actId : null;
}

function query(root, selector) {
  const element = root.querySelector(selector);
  if (!element) throw new Error(`The film needs ${selector}.`);
  return element;
}

// An act's copy comes in as the playhead reaches its resting point and goes
// as it leaves; both on the clock, both undone by scrolling back over them.
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

// Where an act is left: a little past its resting point, so the ease onto
// the resting point never counts as leaving.
const leaveAt = (actId) => between(actId, 0.03);

function copyGates(gates, acts) {
  for (const act of ACTS) {
    const copy = { items: copyItems(acts[act.id - 1]), tween: null };
    // The hero's copy is on the page from the first paint; only leaving and
    // coming back move it.
    if (act.id !== 1) gates.add(arrivalAt(act.id), (instant) => showCopy(copy, instant), () => hideCopy(copy, false, 28));
    if (act.id !== ACTS.length) gates.add(leaveAt(act.id), (instant) => hideCopy(copy, instant, -18), () => showCopy(copy, false, -18));
  }
}

const placement = (rest) => {
  const { x, y, w, yaw, pitch, roll } = fullPose(rest);
  return { x, y, w, yaw, pitch, roll };
};

// Each act's beat: one paused timeline, played by a single eased tween over
// BEAT_SECONDS whatever its storyboard length, so every act slows into its
// conclusion the same way. Acts 2-7 are their scenes. The hero's beat is
// the push-in, the editor close-up and its scene; act 8's is its two copy
// beats, the second bringing the form. `offsets` is where a scene starts
// inside its beat, for sceneSeek. A beat never writes what the scroll
// writes: the push-in is `heroPush`, which the frame applies only while the
// scroll is still in the hero, so a beat finished during a fast jump cannot
// put the laptop back where the jump took it from.
function createBeats({ scenes, heroPush, panels, act8 }) {
  const beats = { ...scenes };
  const offsets = {};
  const hero = gsap.timeline({ paused: true });
  hero.fromTo(heroPush, { t: 0 }, { t: 1, duration: 1.2, ease: "power2.inOut", immediateRender: false }, 0);
  panels.editor.state.shown = 0;
  hero.fromTo(panels.editor.state, { shown: 0 }, { shown: 1, duration: 0.3, immediateRender: false }, 1.1);
  offsets[1] = 1.4;
  hero.add(scenes[1].paused(false), offsets[1]);
  beats[1] = hero;

  const a = query(act8, '[data-beat="a"]');
  const b = query(act8, '[data-beat="b"]');
  const closing = gsap.timeline({ paused: true });
  closing.fromTo(a, { autoAlpha: 0, y: 28 }, { autoAlpha: 1, y: 0, duration: 0.8, ease: "power2.out" }, 0);
  closing.to(a, { autoAlpha: 0, y: -18, duration: 0.4, ease: "power2.in" }, 3.2);
  closing.fromTo(b, { autoAlpha: 0, y: 28 }, { autoAlpha: 1, y: 0, duration: 1.2, ease: "power2.out" }, 3.8);
  beats[8] = closing;
  return { beats, offsets };
}

function createBeatPlayer(beats) {
  const started = new Set();
  const drivers = new Map();
  const halt = (actId) => {
    drivers.get(actId)?.kill();
    drivers.delete(actId);
    beats[actId].pause();
  };
  return {
    started,
    play(actId) {
      started.add(actId);
      halt(actId);
      beats[actId].progress(0);
      drivers.set(actId, gsap.fromTo(beats[actId], { progress: 0 }, { progress: 1, duration: BEAT_SECONDS, ease: BEAT_EASE }));
    },
    finish(actId) {
      started.add(actId);
      halt(actId);
      beats[actId].progress(1);
    },
    rewind(actId) {
      halt(actId);
      beats[actId].progress(0);
    },
    halt,
    playing: (actId) => Boolean(drivers.get(actId)?.isActive()),
    kill() {
      for (const driver of drivers.values()) driver.kill();
      for (const beat of Object.values(beats)) beat.kill();
    },
  };
}

// Reaching an act's resting point plays its beat, the first time; after
// that, or on a page that opens there, the act is shown finished. Leaving,
// either way, finishes a beat still playing, so an act is always finished
// when the visitor comes back to it; leaving forward also plays the
// departure of a close-up that lifted, and crossing back over it plays the
// departure in reverse. Act 8's beat is its copy, so it goes with the copy
// when the visitor scrolls back. The hero's beat starts with the first
// frame instead.
function beatGates(gates, player, departures) {
  for (const act of ACTS) {
    const departure = departures[act.id];
    if (act.id !== 1) {
      gates.add(arrivalAt(act.id), (instant) => {
        if (instant || player.started.has(act.id)) player.finish(act.id);
        else player.play(act.id);
      }, () => {
        if (act.id === ACTS.length) player.rewind(act.id);
        else player.finish(act.id);
      });
    }
    if (act.id === ACTS.length) continue;
    gates.add(leaveAt(act.id), (instant) => {
      player.finish(act.id);
      if (!departure) return;
      if (instant) departure.pause().progress(1);
      else departure.play(0);
    }, () => departure?.reverse());
  }
}

// The devices between resting points. Every move is a fromTo from one rest
// pose to the next, so the scroll and the beats agree on where a device is
// whichever way the visitor goes.
function deviceTimeline(tl, pose) {
  const move = (device, from, to, start, end, ease = "power2.inOut") => {
    tl.fromTo(pose[device], fullPose(from), { ...fullPose(to), duration: end - start, ease, immediateRender: false }, start);
  };
  const laptop = POSES.laptop;
  const laptopRests = [laptop["1-typing"], laptop[2], laptop[3], laptop[4], laptop[5], laptop[6], laptop[7], laptop[8]];
  laptopRests.slice(1).forEach((rest, index) => {
    move("laptop", laptopRests[index], rest, between(index + 1, 0.2), between(index + 1, 0.85));
  });

  const phone = POSES.phone;
  // In place before act 4's copy, so its question is on screen when the
  // beat asks it.
  move("phone", entrancePose("phone", phone[4]), phone[4], between(3, 0.3), between(3, 0.9), "power3.out");
  move("phone", phone[4], departPose("phone", phone[4]), between(4, 0.1), between(4, 0.5), "power2.in");
  move("phone", entrancePose("phone", phone[8]), phone[8], between(7, 0.35), between(7, 0.95), "power3.out");

  const tablet = POSES.tablet;
  move("tablet", entrancePose("tablet", tablet["7-arrive"]), tablet["7-arrive"], between(6, 0.3), between(6, 0.75), "power3.out");
  move("tablet", tablet["7-arrive"], tablet[7], between(6, 0.75), between(6, 0.95));
  move("tablet", tablet[7], tablet[8], between(7, 0.2), between(7, 0.85));
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
      // Wherever the visitor stops, the scroll eases on to the nearest
      // resting point in the direction they were going. The end of the pin
      // is one too, so a stop in act 8's tail is not pulled back up.
      snap: {
        snapTo: [...ACTS.map((act) => restAt(act.id) / TOTAL_TRAVEL), 1],
        directional: true,
        // Snap goes to the next resting point from where the scroll stopped,
        // not where its speed would have carried it.
        inertia: false,
        delay: 0.1,
        duration: { min: 0.35, max: 0.9 },
        ease: "power2.inOut",
      },
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
  const heroPush = { t: 0 };
  const { beats, offsets } = createBeats({ scenes, heroPush, panels: overlays.panels, act8: acts[ACTS.length - 1] });
  const player = createBeatPlayer(beats);
  copyGates(gates, acts);
  beatGates(gates, player, departures);
  // The scrub maps scroll onto the timeline's whole duration; the last beat
  // does not run to the end of act 8, so hold the clock open to it.
  tl.set({}, {}, TOTAL_TRAVEL);

  let stopped = false;
  let firstFrameShown = false;
  // One rendered frame: the displays, the poses and the close-ups all read
  // the same clock, the timeline's, so a fast scroll cannot swap a screen
  // before the overlay that matches it arrives.
  const heroFrom = placement(POSES.laptop[1]);
  const heroTo = placement(POSES.laptop["1-typing"]);
  const draw = () => {
    gates.update(tl.time());
    // Before the first move the laptop is the hero's beat's to place.
    if (tl.time() < between(1, 0.2)) {
      for (const key of Object.keys(heroTo)) pose.laptop[key] = heroFrom[key] + (heroTo[key] - heroFrom[key]) * heroPush.t;
    }
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
    // Before its first frame the film has put nobody anywhere; a destination
    // asked for meanwhile still stands in the document.
    const act = (!firstFrameShown && hashedAct()) || actAt(tl.scrollTrigger.progress * TOTAL_TRAVEL).act;
    gsap.ticker.remove(frame);
    boxWatcher?.disconnect();
    ScrollTrigger.removeEventListener("refresh", sync);
    tl.scrollTrigger.kill();
    tl.kill();
    player.kill();
    for (const departure of Object.values(departures)) departure.kill();
    gsap.set(acts.flatMap((act) => [...copyItems(act), ...act.querySelectorAll("[data-beat], .captions li")]), { clearProps: "all" });
    overlays.dispose();
    stage.dispose();
    delete root.dataset.mode;
    delete root.dataset.stage;
    delete film.dataset.act;
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
    // A page that opens mid-film has its copy and beats where the gates put
    // them on the first frame, the hero's scene finished and the scroll
    // placing its laptop; only the top gets the welcome and the hero's beat.
    // The copy is already on the page and stays put.
    const opening = tl.scrollTrigger.progress * TOTAL_TRAVEL < leaveAt(1);
    if (opening) {
      pose.laptop.lidOpen = LID_ENTRANCE_START;
      draw();
      player.play(1);
    } else {
      player.started.add(1);
      scenes[1].progress(1);
      heroPush.t = 1;
      overlays.panels.editor.state.shown = 1;
    }
    root.dataset.stage = "ready";
    const handover = gsap.timeline();
    handover.fromTo(canvas, { autoAlpha: 0 }, { autoAlpha: 1, duration: HANDOVER_SECONDS, ease: "none" }, 0);
    handover.to(heroPoster, { autoAlpha: 0, duration: HANDOVER_SECONDS, ease: "none" }, 0);
    if (opening) handover.to(pose.laptop, { lidOpen: 1, duration: LID_ENTRANCE_SECONDS, ease: "power3.out" }, HANDOVER_SECONDS);
  }

  // "See how it works" and "Join the waitlist" point at acts, in the film and
  // in the bar; inside the pin an anchor jump lands nowhere useful, so they
  // scroll the film instead, to the act's resting point with its beat
  // already finished: act 8's form is on screen as the visitor arrives.
  const scrollFor = (time) => {
    const trigger = tl.scrollTrigger;
    return trigger.start + (time / TOTAL_TRAVEL) * (trigger.end - trigger.start);
  };
  const goToAct = (actId, behavior) => {
    player.started.add(actId);
    scrollTo({ top: scrollFor(restAt(actId)), behavior });
  };
  document.addEventListener("click", (event) => {
    const anchor = event.target.closest('a[href^="#act-"]');
    if (!anchor || stopped) return;
    const actId = Number(anchor.getAttribute("href").slice(5));
    if (!Number.isInteger(actId) || !ACTS.some((act) => act.id === actId)) return;
    event.preventDefault();
    // Before the first frame the scroll may not be the film's yet; the hash
    // keeps the destination for the first frame, or for the document if the
    // film gives up first.
    if (!firstFrameShown) history.replaceState(null, "", `#act-${actId}`);
    goToAct(actId, "smooth");
  });
  // A click that came before this module did (the hero's call to action on
  // a slow connection), or a link to an act, left its act in the hash; the
  // film opens there rather than at the top, with that act's state set on
  // the first frame instead of played through from the hero.
  function openAtPendingAct() {
    const pendingAct = hashedAct();
    if (!pendingAct) return;
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
      scrollTo({ top: scrollFor(at(actId, local)), behavior: "instant" });
    },
    // The resting point of an act, where its copy and beat are.
    rest(actId) {
      scrollTo({ top: scrollFor(restAt(actId)), behavior: "instant" });
    },
    time: () => tl.scrollTrigger.progress * TOTAL_TRAVEL,
    sync,
    timeline: tl,
    // Each act's beat, and whether its clock is running.
    beats,
    playing: (actId) => player.playing(actId),
    // Put an act's scene at a storyboard beat and hold it there, for a check
    // that wants a beat rather than the clock.
    sceneSeek(actId, local) {
      if (!scenes[actId]) return;
      player.halt(actId);
      const beat = beats[actId];
      beat.time(Math.min(beat.duration(), (offsets[actId] ?? 0) + sceneClock(actId).at(actId, local)));
    },
    stage,
    pose,
    stop,
  };
  window.BuildFilm = film_;
  return film_;
}
