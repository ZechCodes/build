// The film's clock and cast. Pure data and arithmetic: which act runs when,
// where each device sits, which display it shows. GSAP and the stage read it;
// nothing here touches the DOM.

// Pinned travel per act in viewport heights, from DIRECTION.md §2 as amended
// in round 2: scroll only moves the devices between acts, so each act keeps a
// hold after its arrival where nothing on the wheel changes and an overscroll
// does not pull the next act in.
export const ACTS = Object.freeze([
  { id: 1, length: 110, layout: "left" },
  { id: 2, length: 90, layout: "left" },
  { id: 3, length: 120, layout: "left" },
  { id: 4, length: 120, layout: "top" },
  { id: 5, length: 130, layout: "top" },
  { id: 6, length: 120, layout: "top" },
  { id: 7, length: 150, layout: "top" },
  { id: 8, length: 90, layout: "left" },
]);

// Where, in each act, the device has settled and what a person reads begins.
// From there the act's scene runs on the clock, in seconds, not on the wheel:
// a scene's `at(act, local)` maps the storyboard's local progress onto those
// seconds so the beats keep the numbers DIRECTION.md gives them.
export const SCENES = Object.freeze({
  1: { arrive: 0.2, seconds: 6 },
  2: { arrive: 0.22, seconds: 3.5 },
  3: { arrive: 0.2, seconds: 7 },
  4: { arrive: 0.2, seconds: 7.5 },
  5: { arrive: 0.15, seconds: 9.5 },
  6: { arrive: 0.1, seconds: 8.5 },
  7: { arrive: 0.25, seconds: 9.5 },
});

export function sceneClock(actId) {
  const scene = SCENES[actId];
  if (!scene) throw new Error(`act ${actId} has no scene`);
  const scale = scene.seconds / (1 - scene.arrive);
  return {
    at: (act, local) => {
      if (act !== actId) throw new Error(`scene ${actId} cannot place act ${act}`);
      return Math.max(0, local - scene.arrive) * scale;
    },
    span: (act, fromLocal, toLocal) => {
      if (act !== actId) throw new Error(`scene ${actId} cannot span act ${act}`);
      return (toLocal - fromLocal) * scale;
    },
  };
}

// Where the copy of an act comes in and goes out, on the clock, as the
// playhead crosses these; and where the next act's move begins.
export const COPY_IN = 0.03;
export const COPY_OUT = 0.86;

export const TOTAL_TRAVEL = ACTS.reduce((total, act) => total + act.length, 0);

export function actStart(actId) {
  let start = 0;
  for (const act of ACTS) {
    if (act.id === actId) return start;
    start += act.length;
  }
  throw new Error(`unknown act ${actId}`);
}

export function actLength(actId) {
  const act = ACTS.find((candidate) => candidate.id === actId);
  if (!act) throw new Error(`unknown act ${actId}`);
  return act.length;
}

// A timeline position: `local` is 0..1 progress within the act.
export function at(actId, local) {
  return actStart(actId) + actLength(actId) * local;
}

// A span's duration in timeline units.
export function span(actId, fromLocal, toLocal) {
  return actLength(actId) * (toLocal - fromLocal);
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
const HERO = { x: 65, y: 57, w: 52, yaw: -10, pitch: 4 };
const HOST = { x: 17, y: 74, w: 24, yaw: 14, pitch: 3, opacity: 0.9 };
export const POSES = Object.freeze({
  laptop: Object.freeze({
    1: HERO,
    // The push-in while the visitor types: the editor's code pane fills the
    // right of the stage and the copy keeps the left.
    "1-typing": { x: 68, y: 60, w: 62, yaw: -8, pitch: 4 },
    2: { x: 61, y: 58, w: 40, yaw: -12, pitch: 3 },
    3: { x: 67, y: 58, w: 50, yaw: -12, pitch: 2 },
    4: { x: 27, y: 63, w: 38, yaw: 12, pitch: 3 },
    5: { x: 50, y: 66, w: 60, faceCamera: 0 },
    6: { x: 50, y: 62, w: 58, faceCamera: 0 },
    7: HOST,
    8: { x: 53, y: 74, w: 30, yaw: 4, pitch: 8 },
  }),
  phone: Object.freeze({
    4: { x: 83, y: 62, w: 21, yaw: -12, pitch: 2, roll: -2 },
    8: { x: 89, y: 78.5, w: 7.5, yaw: -10, pitch: 2 },
  }),
  tablet: Object.freeze({
    "7-arrive": { x: 66, y: 58, w: 58, yaw: -8, pitch: 3 },
    7: { x: 66, y: 58, w: 58, faceCamera: 0 },
    8: { x: 75, y: 77, w: 24, yaw: -8, pitch: 3 },
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
    [at(3, 0), "ui12-issues-macbook"],
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
