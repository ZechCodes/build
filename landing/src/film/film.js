// The film: one pinned viewport, one scrubbed timeline, three devices and the
// close-ups that sit on their screens. Everything time-based lives here; the
// numbers live in acts.js.
import gsap from "gsap";
import { ScrollTrigger } from "gsap/ScrollTrigger";
import { createDeviceStage } from "../stage/stage.js";
import { STAGE_LIMITS, renderQualityScale } from "../stage/fallback.js";
import {
  ACTS,
  POSES,
  SCREEN_CUES,
  TOTAL_TRAVEL,
  at,
  actAt,
  entrancePose,
  fullPose,
  span,
} from "./acts.js";
import { createScreenResolver, upcomingCues } from "./cues.js";
import { createOverlays } from "./overlays.js";

gsap.registerPlugin(ScrollTrigger);

const SCRUB_SECONDS = 0.7;
const PRELOAD_LOOKAHEAD = 130;
const LID_ENTRANCE_START = 0.7;
const LID_ENTRANCE_SECONDS = 0.9;

function departPose(deviceName, settled) {
  return { ...entrancePose(deviceName, settled), x: settled.x + 14, y: settled.y + 6 };
}

function query(root, selector) {
  const element = root.querySelector(selector);
  if (!element) throw new Error(`The film needs ${selector}.`);
  return element;
}

function copyTimeline(tl, acts) {
  for (const act of ACTS) {
    const copy = query(acts[act.id - 1], ".act__copy");
    const items = [...copy.children].filter((child) => !child.matches("[data-beat]"));
    if (act.id !== 1) {
      tl.fromTo(items, { autoAlpha: 0, y: 28 }, {
        autoAlpha: 1, y: 0, duration: span(act.id, 0, 0.1), stagger: span(act.id, 0, 0.012), ease: "power2.out",
      }, at(act.id, 0.02));
    }
    if (act.id !== 8) {
      tl.to(items, { autoAlpha: 0, y: -18, duration: span(act.id, 0.86, 0.97), ease: "power2.in" }, at(act.id, 0.86));
    }
  }
  // Act 2's captions arrive one at a time, then hold.
  const captions = acts[1].querySelectorAll(".captions li");
  captions.forEach((caption, index) => {
    const start = [0.25, 0.45, 0.65][index] ?? 0.65;
    tl.fromTo(caption, { autoAlpha: 0, y: 16 }, { autoAlpha: 1, y: 0, duration: span(2, 0, 0.08), ease: "power2.out" }, at(2, start));
  });
  // Act 8's two beats over one pose.
  const beatA = query(acts[7], '[data-beat="a"]');
  const beatB = query(acts[7], '[data-beat="b"]');
  tl.fromTo(beatA, { autoAlpha: 0, y: 28 }, { autoAlpha: 1, y: 0, duration: span(8, 0, 0.1), ease: "power2.out" }, at(8, 0.02));
  tl.to(beatA, { autoAlpha: 0, y: -18, duration: span(8, 0.6, 0.68), ease: "power2.in" }, at(8, 0.6));
  tl.fromTo(beatB, { autoAlpha: 0, y: 28 }, { autoAlpha: 1, y: 0, duration: span(8, 0.66, 0.78), ease: "power2.out" }, at(8, 0.66));
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
  move("phone", phone[4], at(4, 0.3), at(4, 0.45), "power3.out");
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
  return (time) => {
    for (const [device, cues] of Object.entries(SCREEN_CUES)) {
      for (const name of upcomingCues(cues, time, PRELOAD_LOOKAHEAD)) {
        const key = `${device}:${name}`;
        if (preloaded.has(key)) continue;
        preloaded.add(key);
        stage.preloadScreen(device, name).catch(() => preloaded.delete(key));
      }
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
    throw error;
  }

  const pose = {
    laptop: fullPose(POSES.laptop[1]),
    phone: fullPose(entrancePose("phone", POSES.phone[4])),
    tablet: fullPose(entrancePose("tablet", POSES.tablet["7-arrive"])),
  };
  const overlays = createOverlays({ film, stage, pose });
  const resolveScreens = createScreenResolver(SCREEN_CUES, (device, name) => {
    stage.setScreen(device, name).catch(() => undefined);
  });
  const preload = screenPreloader(stage);

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
  copyTimeline(tl, acts);
  overlays.addTo(tl);
  // The scrub maps scroll onto the timeline's whole duration; the last beat
  // does not run to the end of act 8, so hold the clock open to it.
  tl.set({}, {}, TOTAL_TRAVEL);

  let stopped = false;
  let firstFrameShown = false;
  // One rendered frame: the displays, the poses and the close-ups all read
  // the same clock, the timeline's, so a fast scroll cannot swap a screen
  // before the overlay that matches it arrives.
  const draw = () => {
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
    overlays.dispose();
    stage.dispose();
    delete root.dataset.mode;
    delete film.dataset.act;
    acts[act - 1].scrollIntoView({ block: "start", behavior: "instant" });
  }

  // The lid does the last quarter as a welcome: it plays once, on the first
  // real frame, only when the page opens at the top. A restored scroll
  // position skips it, and the poster is on screen until the frame exists.
  function showFirstFrame() {
    if (firstFrameShown) return;
    firstFrameShown = true;
    const cost = stage.measureFrameCost();
    if (!ignoreFrameBudget && cost > STAGE_LIMITS.slowFrameMs) {
      stop(`frame budget: ${cost.toFixed(1)}ms`);
      return;
    }
    stage.setQualityScale(renderQualityScale(cost));
    root.dataset.stage = "ready";
    gsap.to(heroPoster, { autoAlpha: 0, duration: 0.45, ease: "power1.out" });
    const copy = query(acts[0], ".act__copy");
    const opening = tl.scrollTrigger.progress * TOTAL_TRAVEL < at(1, 0.15);
    if (opening) {
      pose.laptop.lidOpen = LID_ENTRANCE_START;
      gsap.to(pose.laptop, { lidOpen: 1, duration: LID_ENTRANCE_SECONDS, ease: "power3.out" });
      gsap.fromTo(copy.children, { autoAlpha: 0, y: 24 }, {
        autoAlpha: 1, y: 0, duration: 0.8, stagger: 0.08, ease: "power2.out", delay: 0.15,
      });
    } else {
      gsap.set(copy.children, { autoAlpha: 1, y: 0 });
    }
  }

  // "See how it works" and "Join the waitlist" point at acts; inside the pin
  // an anchor jump lands nowhere useful, so they scroll the film instead.
  film.addEventListener("click", (event) => {
    const anchor = event.target.closest('a[href^="#act-"]');
    if (!anchor || stopped) return;
    const actId = Number(anchor.getAttribute("href").slice(5));
    if (!Number.isInteger(actId)) return;
    event.preventDefault();
    const local = actId === 8 ? 0.72 : 0.08;
    const trigger = tl.scrollTrigger;
    scrollTo({ top: trigger.start + (at(actId, local) / TOTAL_TRAVEL) * (trigger.end - trigger.start), behavior: "smooth" });
  });

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
    stage,
    pose,
    stop,
  };
  window.BuildFilm = film_;
  return film_;
}
