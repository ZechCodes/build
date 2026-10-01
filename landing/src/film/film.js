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

function copyItems(act) {
  const copy = query(act, ".act__copy");
  return [...copy.children].filter((child) => !child.matches("[data-beat]"));
}

// Where an act is left: a little past its resting point, so the ease onto
// the resting point never counts as leaving.
const leaveAt = (actId) => between(actId, 0.03);

// An act's copy is on the scroll: it comes in over the last stretch of the
// move into the act, so a visitor still scrolling sees it arrive and knows
// to stop, and goes early in the move out. Between the two, mid-move, the
// devices have the stage to themselves. Act 8's first copy beat comes in
// with its copy; its beat brings the second.
const COPY_IN = [0.6, 0.95];
const COPY_OUT = [0.1, 0.35];

function copyTimeline(tl, acts) {
  for (const act of ACTS) {
    const element = acts[act.id - 1];
    const items = copyItems(element);
    if (act.id === ACTS.length) items.push(query(element, '[data-beat="a"]'));
    if (act.id !== 1) {
      const [from, to] = COPY_IN.map((fraction) => between(act.id - 1, fraction));
      tl.fromTo(items, { autoAlpha: 0, y: 28 }, { autoAlpha: 1, y: 0, duration: (to - from) * 0.8, stagger: { amount: (to - from) * 0.2 }, ease: "power2.out", immediateRender: false }, from);
    }
    if (act.id !== ACTS.length) {
      const [from, to] = COPY_OUT.map((fraction) => between(act.id, fraction));
      tl.fromTo(items, { autoAlpha: 1, y: 0 }, { autoAlpha: 0, y: -18, duration: to - from, ease: "power2.in", immediateRender: false }, from);
    }
  }
}

// Each act's beat: one paused timeline, played by a single eased tween over
// BEAT_SECONDS whatever its storyboard length, so every act slows into its
// conclusion the same way. Acts 2-7 are their scenes; act 8's is its two
// copy beats, the second bringing the form. The hero has its own entrance
// (src/hero/), once per tab and on its own clock, so its beat here is
// empty. `offsets` is where a scene starts inside its beat, for sceneSeek.
function createBeats({ scenes, act8 }) {
  const beats = { ...scenes };
  const offsets = {};
  beats[1] = gsap.timeline({ paused: true });

  const a = query(act8, '[data-beat="a"]');
  const b = query(act8, '[data-beat="b"]');
  const closing = gsap.timeline({ paused: true });
  // The first copy beat is the scroll's to bring in; the beat holds it, then
  // takes its lines away. The beat fades the lines, never the element the
  // scroll fades, so a rewind cannot undo the scroll.
  closing.to([...a.children], { autoAlpha: 0, y: -18, duration: 0.4, ease: "power2.in" }, 3.2);
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

// Reaching an act's resting point, with its copy fully in, plays its beat, the first time; after
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
  const laptopRests = [laptop[1], laptop[2], laptop[3], laptop[4], laptop[5], laptop[6], laptop[7], laptop[8]];
  laptopRests.slice(1).forEach((rest, index) => {
    move("laptop", laptopRests[index], rest, between(index + 1, 0.2), between(index + 1, 0.85));
  });

  // A remote screen comes in only after its act's display cue, at the middle
  // of the move (SCREEN_CUES), so it never shows a blank screen; and it is
  // in place before the act's copy is fully in, so act 4's question is on
  // screen when the beat asks it.
  const phone = POSES.phone;
  move("phone", entrancePose("phone", phone[4]), phone[4], between(3, 0.52), between(3, 0.92), "power3.out");
  move("phone", phone[4], departPose("phone", phone[4]), between(4, 0.1), between(4, 0.45), "power2.in");
  move("phone", entrancePose("phone", phone[8]), phone[8], between(7, 0.52), between(7, 0.95), "power3.out");

  const tablet = POSES.tablet;
  move("tablet", entrancePose("tablet", tablet["7-arrive"]), tablet["7-arrive"], between(6, 0.52), between(6, 0.8), "power3.out");
  move("tablet", tablet["7-arrive"], tablet[7], between(6, 0.8), between(6, 0.95));
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

// The hero's laptop, while its entrance plays: the stage draws the turn in if
// it is ready before the turn begins, otherwise the picture does and the
// stage takes over once the hero is still. Returns how the stage comes in.
function handHeroLaptop(hero, stage, pose) {
  if (!hero) return "handover";
  return hero.attachStage({
    pose: pose.laptop,
    final: fullPose(POSES.laptop[1]),
    corners: (laptopPose) => stage.screenCorners("laptop", laptopPose),
  });
}

export function startFilm({ ignoreFrameBudget = false, hero = null } = {}) {
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
  const { beats, offsets } = createBeats({ scenes, act8: acts[ACTS.length - 1] });
  const player = createBeatPlayer(beats);
  copyTimeline(tl, acts);
  beatGates(gates, player, departures);
  // The scrub maps scroll onto the timeline's whole duration; the last beat
  // does not run to the end of act 8, so hold the clock open to it.
  tl.set({}, {}, TOTAL_TRAVEL);

  let stopped = false;
  let firstFrameShown = false;
  // One rendered frame: the displays, the poses and the close-ups all read
  // the same clock, the timeline's, so a fast scroll cannot swap a screen
  // before the overlay that matches it arrives. Before the first move the
  // laptop is the hero entrance's to place, when it is playing.
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
  // A resize re-measures the pin, and the same pixel offset would then be
  // somewhere else in the film; keep the visitor where they were instead.
  let heldTime = null;
  // Only once the film is running: before its first frame a restored or
  // pending position is still the page's to set.
  const holdPlace = () => { heldTime = firstFrameShown ? tl.scrollTrigger.progress * TOTAL_TRAVEL : null; };
  const keepPlace = () => {
    if (heldTime === null || stopped) return;
    const time = heldTime;
    heldTime = null;
    goTo(time, "instant");
  };
  ScrollTrigger.addEventListener("refreshInit", holdPlace);
  ScrollTrigger.addEventListener("refresh", keepPlace);
  ScrollTrigger.addEventListener("refresh", sync);

  function stop(reason) {
    if (stopped) return;
    stopped = true;
    console.info(`The film stops (${reason}); the document stands.`);
    // The hero was laid out for the film: it rests in the document's layout,
    // with its picture, whatever the stage was drawing.
    hero?.finish("film stopped");
    gsap.set(heroPoster, { clearProps: "opacity,visibility" });
    // Before its first frame the film has put nobody anywhere; a destination
    // asked for meanwhile still stands in the document.
    const act = (!firstFrameShown && hashedAct()) || actAt(tl.scrollTrigger.progress * TOTAL_TRAVEL).act;
    gsap.ticker.remove(frame);
    boxWatcher?.disconnect();
    ScrollTrigger.removeEventListener("refresh", sync);
    ScrollTrigger.removeEventListener("refreshInit", holdPlace);
    ScrollTrigger.removeEventListener("refresh", keepPlace);
    tl.scrollTrigger.kill();
    tl.kill();
    player.kill();
    for (const departure of Object.values(departures)) departure.kill();
    gsap.set(acts.flatMap((act) => [...copyItems(act), ...act.querySelectorAll("[data-beat], [data-beat] > *, .captions li")]), { clearProps: "all" });
    overlays.dispose();
    stage.dispose();
    delete root.dataset.mode;
    delete root.dataset.stage;
    delete film.dataset.act;
    acts[act - 1].scrollIntoView({ block: "start", behavior: "instant" });
  }

  // Until the stage's first frame the hero's picture holds its place: a
  // capture of this stage at the hero's resting pose. If the hero's
  // entrance is still to turn the laptop in, the stage draws the turn;
  // otherwise the stage takes over from the picture once the hero is still,
  // a crossfade between two pictures of the same thing. A page that opens
  // mid-film has its copy and beats where the gates put them.
  function showFirstFrame() {
    if (firstFrameShown) return;
    firstFrameShown = true;
    const cost = stage.measureFrameCost();
    if (!ignoreFrameBudget && cost > STAGE_LIMITS.slowFrameMs) {
      stop(`frame budget: ${cost.toFixed(1)}ms`);
      return;
    }
    stage.setQualityScale(renderQualityScale(cost));
    player.started.add(1);
    root.dataset.stage = "ready";
    if (handHeroLaptop(hero, stage, pose) === "drive") {
      gsap.set(canvas, { autoAlpha: 1 });
      return;
    }
    const handover = () => {
      if (stopped) return;
      const crossfade = gsap.timeline();
      crossfade.fromTo(canvas, { autoAlpha: 0 }, { autoAlpha: 1, duration: HANDOVER_SECONDS, ease: "none" }, 0);
      crossfade.to(heroPoster, { autoAlpha: 0, duration: HANDOVER_SECONDS, ease: "none" }, 0);
    };
    if (hero) hero.whenSettled(handover);
    else handover();
  }

  // "See how it works" and "Join the waitlist" point at acts, in the film and
  // in the bar; inside the pin an anchor jump lands nowhere useful, so they
  // scroll the film instead, to the act's resting point with its beat
  // already finished: act 8's form is on screen as the visitor arrives.
  const scrollFor = (time) => {
    const trigger = tl.scrollTrigger;
    return trigger.start + (time / TOTAL_TRAVEL) * (trigger.end - trigger.start);
  };
  // Somewhere in the film, deliberately: a snap still easing toward the last
  // resting point would otherwise carry the page back there.
  function goTo(time, behavior) {
    // ScrollTrigger leaves 0 here once a snap is done.
    const snap = tl.scrollTrigger.getTween(true);
    if (snap) snap.kill();
    scrollTo({ top: scrollFor(time), behavior });
  }
  const goToAct = (actId, behavior) => {
    player.started.add(actId);
    goTo(restAt(actId), behavior);
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
      goTo(at(actId, local), "instant");
    },
    // The resting point of an act, where its copy and beat are.
    rest(actId) {
      goTo(restAt(actId), "instant");
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
