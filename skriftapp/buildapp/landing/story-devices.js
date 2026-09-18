import { getDeviceFramePoses, installDeviceStage } from "./device-stage.js";
import { STORY_SCENES } from "./story-manifest.js";

const DEVICE_NAMES = ["laptop", "tablet", "phone"];

function devicesNeeded(frame) {
  if (frame.sceneIndex >= 3) return DEVICE_NAMES;
  if (frame.sceneIndex >= 1 || frame.local > 0.65) return ["laptop", "phone"];
  return ["laptop"];
}

function loadDevicePosters(deviceElements, frame) {
  for (const name of devicesNeeded(frame)) {
    const element = deviceElements[name];
    if (element && !element.src && element.dataset.src) element.src = element.dataset.src;
  }
}

function reviewWidthPercent(storyStage, reviewSurface) {
  const stageWidth = storyStage.getBoundingClientRect().width;
  if (stageWidth <= 0 || !reviewSurface) return undefined;
  return reviewSurface.getBoundingClientRect().width / stageWidth * 100;
}

function applyPosterPose(element, pose) {
  if (!element) return;
  const [yaw, pitch, roll] = pose.rotate;
  const properties = {
    "--device-x": pose.x,
    "--device-y": pose.y,
    "--device-w": pose.w,
    "--device-yaw": yaw,
    "--device-pitch": pitch,
    "--device-roll": roll,
    "--device-opacity": pose.opacity,
  };
  for (const [property, value] of Object.entries(properties)) {
    element.style.setProperty(property, value);
  }
}

function updateDevicePosters({ deviceElements, frame, reviewSurface, storyStage }) {
  loadDevicePosters(deviceElements, frame);
  const reviewWidth = reviewWidthPercent(storyStage, reviewSurface);
  const poses = getDeviceFramePoses(STORY_SCENES, frame, { reviewWidth });
  for (const [name, pose] of Object.entries(poses)) applyPosterPose(deviceElements[name], pose);
}

// Posters follow every story frame; the WebGL models sit on top of them while
// they can. If the renderer never starts, loses its context or falls behind,
// the models go away and the posters carry the story on their own.
export function installStoryDevices({ story, storyStage, deviceStage }) {
  const deviceElements = Object.fromEntries(
    DEVICE_NAMES.map((name) => [name, deviceStage.querySelector(`[data-device="${name}"]`)]),
  );
  const reviewSurface = storyStage.querySelector("[data-review-surface]");
  story.subscribe((frame) => updateDevicePosters({ deviceElements, frame, reviewSurface, storyStage }));
  return installDeviceStage(deviceStage, { story, scenes: STORY_SCENES, eventTarget: storyStage });
}
