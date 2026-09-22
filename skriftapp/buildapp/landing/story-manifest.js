const hidden = Object.freeze({ x: 50, y: 62, w: 10, rotate: [0, 0, 0], opacity: 0 });

const pose = (x, y, w, rotate = [0, 0, 0], opacity = 1) =>
  Object.freeze({ x, y, w, rotate: Object.freeze(rotate), opacity });

const devices = (laptop = hidden, tablet = hidden, phone = hidden, review = hidden) =>
  Object.freeze({ laptop, tablet, phone, review });

export const STORY_SCENES = Object.freeze([
  {
    id: "start",
    travel: { desktop: 0.7, tablet: 0.7, compact: 0.5 },
    copy: { title: "Set the work in motion.", region: "left" },
    checkpoints: [{ at: 0, id: "request" }, { at: 0.42, id: "agents-started" }, { at: 0.85, id: "running" }],
    poses: {
      desktop: devices(pose(65, 57, 52, [0, 4, 0])),
      tablet: devices(pose(52, 60, 64, [0, 4, 0])),
      compact: devices(pose(50, 68, 92)),
    },
  },
  {
    id: "handoff",
    travel: { desktop: 0.75, tablet: 0.7, compact: 0.5 },
    copy: { title: "Your day moves.", region: "left" },
    checkpoints: [{ at: 0, id: "laptop" }, { at: 0.25, id: "inbox" }, { at: 0.75, id: "handoff-ready" }],
    poses: {
      desktop: devices(pose(30, 66, 32, [12, 3, 0], 0.58), hidden, pose(83, 56, 21, [-12, 2, -2])),
      tablet: devices(pose(24, 70, 42, [5, 2, 0], 0.52), hidden, pose(77, 66, 24, [-4, 1, -1])),
      compact: devices(pose(20, 64, 50, [0, 0, 0], 0.42), hidden, pose(58, 68, 58, [-5, 0, 0])),
    },
  },
  {
    id: "direction",
    travel: { desktop: 0.7, tablet: 0.65, compact: 0.55 },
    copy: { title: "A little direction. Back to work.", region: "left" },
    checkpoints: [{ at: 0, id: "question" }, { at: 0.42, id: "answer" }, { at: 0.68, id: "resumed" }],
    poses: {
      desktop: devices(hidden, hidden, pose(84, 56, 22, [-10, 3, 0])),
      tablet: devices(hidden, hidden, pose(75, 64, 26, [-8, 3, 0])),
      compact: devices(hidden, hidden, pose(50, 68, 64)),
    },
  },
  {
    id: "overview",
    travel: { desktop: 0.75, tablet: 0.7, compact: 0.55 },
    copy: { title: "See the whole picture.", region: "left" },
    checkpoints: [{ at: 0, id: "workspace" }, { at: 0.3, id: "activity" }, { at: 0.75, id: "review-approach" }],
    poses: {
      desktop: devices(hidden, pose(73, 57, 50, [-12, 5, 0])),
      tablet: devices(hidden, pose(50, 78, 62, [-8, 4, 0])),
      compact: devices(hidden, pose(50, 70, 94)),
    },
  },
  {
    id: "review",
    travel: { desktop: 1.1, tablet: 0.9, compact: 0.7 },
    copy: { title: "Keep the final say.", region: "top" },
    checkpoints: [
      { at: 0, id: "align" },
      { at: 0.2, id: "summary" },
      { at: 0.35, id: "diff" },
      { at: 0.5, id: "evidence" },
      { at: 0.65, id: "approval" },
      { at: 0.8, id: "merged" },
    ],
    poses: {
      desktop: devices(hidden, hidden, hidden, pose(50, 61, 82)),
      tablet: devices(hidden, hidden, hidden, pose(50, 62, 90)),
      compact: devices(hidden, hidden, hidden, pose(50, 66, 92)),
    },
  },
  {
    id: "download",
    travel: { desktop: 0.5, tablet: 0.45, compact: 0.45 },
    copy: { title: "Any screen. Your call.", region: "center" },
    checkpoints: [{ at: 0, id: "arrive" }, { at: 0.35, id: "settled" }],
    poses: {
      desktop: devices(pose(32, 72, 34, [4, 8, 0]), pose(58, 75.8, 27.2, [-8, 3, 0]), pose(77.8, 77.1, 8.5, [-10, 2, 0])),
      tablet: devices(pose(27, 74, 42, [4, 8, 0]), pose(57, 76.2, 33.55, [-8, 3, 0]), pose(80, 77, 10.48, [-10, 2, 0])),
      compact: devices(pose(26, 77, 48), pose(58, 79, 38.34), pose(83, 80, 11.98)),
    },
  },
]);

export const STORY_PROFILES = Object.freeze({
  desktop: Object.freeze({ minWidth: 1100, minHeight: 700 }),
  tablet: Object.freeze({ minWidth: 768, minHeight: 600 }),
  compact: Object.freeze({ minWidth: 0, minHeight: 600 }),
});

export function clamp(value, minimum = 0, maximum = 1) {
  return Math.min(maximum, Math.max(minimum, value));
}

// Phones read the story as a document: a pinned stage narrower than a tablet
// cannot hold a headline, a device and a demonstration card at once.
export function profileForViewport(width, height) {
  if (height < STORY_PROFILES.tablet.minHeight || width < STORY_PROFILES.tablet.minWidth) return "static";
  if (width >= STORY_PROFILES.desktop.minWidth && height >= STORY_PROFILES.desktop.minHeight) return "desktop";
  return "tablet";
}

export function storyTravel(profile) {
  if (profile === "static") return 0;
  return STORY_SCENES.reduce((total, scene) => total + scene.travel[profile], 0);
}

export function checkpointAt(scene, local) {
  return scene.checkpoints.reduce(
    (current, checkpoint) => (local >= checkpoint.at ? checkpoint.id : current),
    scene.checkpoints[0].id,
  );
}

export function frameAtTravel(travel, profile) {
  const safeProfile = profile === "static" ? "compact" : profile;
  const total = storyTravel(safeProfile);
  const boundedTravel = clamp(travel, 0, total);
  let traversed = 0;

  for (let sceneIndex = 0; sceneIndex < STORY_SCENES.length; sceneIndex += 1) {
    const scene = STORY_SCENES[sceneIndex];
    const sceneTravel = scene.travel[safeProfile];
    const isLast = sceneIndex === STORY_SCENES.length - 1;
    if (boundedTravel <= traversed + sceneTravel || isLast) {
      const local = clamp((boundedTravel - traversed) / sceneTravel);
      return {
        scene: scene.id,
        sceneIndex,
        local,
        progress: total === 0 ? 0 : boundedTravel / total,
        profile: safeProfile,
        checkpoint: checkpointAt(scene, local),
        travel: boundedTravel,
      };
    }
    traversed += sceneTravel;
  }

  return null;
}

export function travelAtFrame(sceneIndex, local, profile) {
  const safeProfile = profile === "static" ? "compact" : profile;
  const preceding = STORY_SCENES.slice(0, sceneIndex).reduce(
    (total, scene) => total + scene.travel[safeProfile],
    0,
  );
  return preceding + STORY_SCENES[sceneIndex].travel[safeProfile] * clamp(local);
}
