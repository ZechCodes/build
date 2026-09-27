// The film's clock and cast. Pure data and arithmetic: which act runs when,
// where each device sits, which display it shows. GSAP and the stage read it;
// nothing here touches the DOM.

// Every act has one resting point: the scroll position where its devices
// have arrived, its copy comes in and its beat plays. Scroll between two
// resting points only moves the devices, and it is the same distance every
// time, so the rhythm is learnable by the second act. `rest` is the resting
// point's local position in the act; the hero rests at the top of the page
// and act 8 keeps a short tail before the film lets go of the page.
export const TRAVEL = 100;
export const ACTS = Object.freeze([
  { id: 1, length: 50, rest: 0, layout: "left" },
  { id: 2, length: 100, rest: 0.5, layout: "left" },
  { id: 3, length: 100, rest: 0.5, layout: "left" },
  { id: 4, length: 100, rest: 0.5, layout: "top" },
  { id: 5, length: 100, rest: 0.5, layout: "top" },
  { id: 6, length: 100, rest: 0.5, layout: "top" },
  { id: 7, length: 100, rest: 0.5, layout: "top" },
  { id: 8, length: 60, rest: 50 / 60, layout: "left" },
]);

// Every beat runs this long from the copy's arrival and ends still: the
// stillness is what says "scroll on". One ease drives each beat's clock, so
// every act slows into its conclusion the same way.
export const BEAT_SECONDS = 5;
export const BEAT_EASE = "sine.out";

// Each act's beat is a storyboard scene: `arrive` and the locals its beats
// are written in come from DIRECTION.md, `seconds` and `holds` give their
// proportions. The film plays every scene in BEAT_SECONDS whatever its
// storyboard length. A `hold` ([local, seconds]) is reading time added after
// that beat.
export const SCENES = Object.freeze({
  1: { arrive: 0.2, seconds: 6 },
  2: { arrive: 0.22, seconds: 3.5 },
  3: { arrive: 0.2, seconds: 7 },
  4: { arrive: 0.2, seconds: 9 },
  5: { arrive: 0.15, seconds: 9.5 },
  6: { arrive: 0.1, seconds: 8.5 },
  // The finding and its evidence are open from .55; the hold keeps them up
  // for a larger share of the beat before the approval comes up at .66.
  7: { arrive: 0.25, seconds: 9.5, holds: [[0.6, 2.7]] },
});

export function sceneClock(actId) {
  const scene = SCENES[actId];
  if (!scene) throw new Error(`act ${actId} has no scene`);
  const scale = scene.seconds / (1 - scene.arrive);
  const holds = scene.holds ?? [];
  const held = (local) => holds.reduce((total, [after, seconds]) => total + (local > after ? seconds : 0), 0);
  return {
    at: (act, local) => {
      if (act !== actId) throw new Error(`scene ${actId} cannot place act ${act}`);
      return Math.max(0, local - scene.arrive) * scale + held(local);
    },
    // A duration: a span of locals, with any hold that falls inside it.
    span: (act, fromLocal, toLocal) => {
      if (act !== actId) throw new Error(`scene ${actId} cannot span act ${act}`);
      return (toLocal - fromLocal) * scale + held(toLocal) - held(fromLocal);
    },
  };
}

// A scene's whole storyboard length in seconds, holds included.
export function sceneSeconds(actId) {
  return sceneClock(actId).at(actId, 1);
}

export const TOTAL_TRAVEL = ACTS.reduce((total, act) => total + act.length, 0);

function actOf(actId) {
  const act = ACTS.find((candidate) => candidate.id === actId);
  if (!act) throw new Error(`unknown act ${actId}`);
  return act;
}

export function actStart(actId) {
  let start = 0;
  for (const act of ACTS) {
    if (act.id === actId) return start;
    start += act.length;
  }
  throw new Error(`unknown act ${actId}`);
}

export function actLength(actId) {
  return actOf(actId).length;
}

// A timeline position: `local` is 0..1 progress within the act.
export function at(actId, local) {
  return actStart(actId) + actLength(actId) * local;
}

// A span's duration in timeline units.
export function span(actId, fromLocal, toLocal) {
  return actLength(actId) * (toLocal - fromLocal);
}

// An act's resting point on the timeline.
export function restAt(actId) {
  return at(actId, actOf(actId).rest);
}

// A point in the move from act `fromAct`'s resting point to the next one's,
// `fraction` 0..1 of the way.
export function between(fromAct, fraction) {
  if (fromAct >= ACTS.length) throw new Error(`act ${fromAct} has no next act`);
  return restAt(fromAct) + (restAt(fromAct + 1) - restAt(fromAct)) * fraction;
}

// A resting point counts as reached a hair before it, since a scroll
// position is whole pixels and the resting point need not be.
export const ARRIVAL_TOLERANCE = 0.5;

export function arrivalAt(actId) {
  return restAt(actId) - ARRIVAL_TOLERANCE;
}

// Which act, and how far into it, a timeline position is.
export function actAt(time) {
  const bounded = Math.min(TOTAL_TRAVEL, Math.max(0, time));
  let start = 0;
  for (const act of ACTS) {
    if (bounded < start + act.length || act.id === ACTS.length) {
      return { act: act.id, local: Math.min(1, (bounded - start) / act.length) };
    }
    start += act.length;
  }
  return { act: ACTS.length, local: 1 };
}

// The devices at rest in each act, desktop profile. A device absent from an
// act is hidden. `opacity` defaults to 1, `lidOpen` to 1, `faceCamera` to 1.
// The hero turns a little toward the copy on the left, not the visitor.
const HERO = { x: 65, y: 53, w: 52, yaw: -10, pitch: 4 };
const HOST = { x: 17, y: 74, w: 24, yaw: 14, pitch: 3, opacity: 0.9 };
export const POSES = Object.freeze({
  laptop: Object.freeze({
    1: HERO,
    // The push-in while the visitor types: the editor's code pane fills the
    // right of the stage and the copy keeps the left.
    "1-typing": { x: 68, y: 57, w: 62, yaw: -8, pitch: 4 },
    2: { x: 61, y: 58, w: 40, yaw: -12, pitch: 3 },
    3: { x: 67, y: 58, w: 50, yaw: -12, pitch: 2 },
    // Act 4 is a pair: the host and, close to the middle, the phone that
    // answers for it.
    4: { x: 35, y: 65, w: 40, yaw: 12, pitch: 3 },
    5: { x: 50, y: 66, w: 60, faceCamera: 0 },
    6: { x: 50, y: 62, w: 58, faceCamera: 0 },
    7: HOST,
    8: { x: 53.75, y: 68, w: 24.5, yaw: 4, pitch: 8 },
  }),
  phone: Object.freeze({
    4: { x: 64, y: 65, w: 17, yaw: -12, pitch: 2, roll: -2 },
    8: { x: 92.65, y: 72.5, w: 6.1, yaw: -10, pitch: 2 },
  }),
  tablet: Object.freeze({
    "7-arrive": { x: 66, y: 62, w: 54, yaw: -8, pitch: 3 },
    7: { x: 66, y: 62, w: 54, faceCamera: 0 },
    8: { x: 77.8, y: 71, w: 19.6, yaw: -8, pitch: 3 },
  }),
});

// How a remote screen comes in: offset, smaller, turned, transparent. The
// old stage's entrance numbers, kept because they read as a pivot rather than
// a slide.
export const ENTRANCES = Object.freeze({
  phone: { x: 13, y: 10, scale: 0.72, yaw: 28, pitch: -8, roll: 8 },
  tablet: { x: 18, y: 12, scale: 0.78, yaw: 26, pitch: -6, roll: 6 },
});

export function entrancePose(deviceName, settled) {
  const offset = ENTRANCES[deviceName];
  if (!offset) return { ...settled, opacity: 0 };
  return {
    ...settled,
    x: settled.x + offset.x,
    y: settled.y + offset.y,
    w: settled.w * offset.scale,
    yaw: (settled.yaw || 0) + offset.yaw,
    pitch: (settled.pitch || 0) + offset.pitch,
    roll: (settled.roll || 0) + offset.roll,
    opacity: 0,
  };
}

// A full pose from a rest pose: every field present, so a tween has a value
// for each.
export function fullPose(pose = {}) {
  return {
    x: pose.x ?? 50,
    y: pose.y ?? 58,
    w: pose.w ?? 0,
    yaw: pose.yaw ?? 0,
    pitch: pose.pitch ?? 0,
    roll: pose.roll ?? 0,
    opacity: pose.opacity ?? (pose.w ? 1 : 0),
    lidOpen: pose.lidOpen ?? 1,
    faceCamera: pose.faceCamera ?? 1,
  };
}

// What each display shows from a given moment on, in timeline units. Resolved
// by cues.js so that scrubbing backwards restores the earlier display. A swap
// that belongs to a scene (the phone's answer, the merge) is not here: the
// scene sets it on the clock, scoped to its act, and SCENE_SCREENS names it
// so it can be preloaded.
export const SCREEN_CUES = Object.freeze({
  laptop: [
    [at(1, 0), "ui10-editor-macbook"],
    [at(3, 0), "ui12-tasks-macbook"],
    [at(4, 0), "ui13-team-macbook"],
    [at(5, 0), "ui16-builder-macbook"],
    [at(6, 0), "ui14-git-macbook"],
    [at(8, 0), "ui05-merged-macbook"],
  ],
  phone: [
    [at(4, 0), "ui03-question-iphone"],
    [at(8, 0), "ui05-merged-iphone"],
  ],
  tablet: [
    [at(7, 0), "ui15-triage-ipad"],
    [at(8, 0), "ui05-merged-ipad"],
  ],
});

export const SCENE_SCREENS = Object.freeze({
  4: { phone: ["ui03-answer-iphone", "ui03-resumed-iphone"] },
  7: { laptop: ["ui05-merged-macbook"], tablet: ["ui05-approval-ipad", "ui05-merged-ipad"] },
});

// Native texture sizes, the coordinate space every close-up is authored in.
export const TEXTURE_SIZES = Object.freeze({
  laptop: [1512, 982],
  tablet: [1210, 834],
  phone: [440, 956],
});
