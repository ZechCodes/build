import { installNavSolidityObserver } from "./nav-solidity.js";
import { installMobileMenu } from "./mobile-menu.js";
import { installDotField } from "./dot-field.js";
import { installFeatureRail } from "./feature-rail.js";
import { installWaitlist } from "./waitlist-form.js";
import { submitWaitlistEmail } from "./waitlist-api.js";
import { installCinematicStory } from "./cinematic-story.js";
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

const navElement = document.querySelector("[data-nav]");
const heroElement = document.querySelector("[data-hero]");
const canvasElement = document.querySelector("[data-dot-field]");
const toggleButton = document.querySelector("[data-nav-toggle]");
const menuElement = document.querySelector("[data-nav-menu]");
const railElement = document.querySelector("[data-feature-rail]");
const cardElements = Array.from(document.querySelectorAll("[data-feature-card]"));
const dashElements = Array.from(document.querySelectorAll("[data-rail-dash]"));
const waitlistWrappers = Array.from(document.querySelectorAll("[data-waitlist]"));
const storyElement = document.querySelector("[data-story]");
const storyStage = document.querySelector("[data-story-stage]");
const deviceStage = document.querySelector("[data-device-stage]");

if (navElement && heroElement) installNavSolidityObserver({ navElement, heroElement });
if (toggleButton && menuElement) installMobileMenu({ toggleButton, menuElement });
if (toggleButton && menuElement) {
  toggleButton.addEventListener("click", () => {
    toggleButton.setAttribute(
      "aria-label",
      toggleButton.getAttribute("aria-expanded") === "true" ? "Close menu" : "Open menu",
    );
  });
  menuElement.addEventListener("click", () => toggleButton.setAttribute("aria-label", "Open menu"));
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape" || toggleButton.getAttribute("aria-expanded") !== "true") return;
    menuElement.classList.remove("is-open");
    toggleButton.setAttribute("aria-expanded", "false");
    toggleButton.setAttribute("aria-label", "Open menu");
    toggleButton.focus();
  });
}
if (canvasElement) installDotField(canvasElement);
if (railElement && cardElements.length && dashElements.length) {
  installFeatureRail({ railElement, cardElements, dashElements });
}
for (const wrapperElement of waitlistWrappers) {
  installWaitlist({ wrapperElement, submitWaitlistEmail });
}

if (storyElement && storyStage && deviceStage) {
  const story = installCinematicStory({ story: storyElement, stage: storyStage });
  const deviceElements = Object.fromEntries(
    DEVICE_NAMES.map((name) => [
      name,
      deviceStage.querySelector(`[data-device="${name}"]`),
    ]),
  );
  const reviewSurface = storyStage.querySelector("[data-review-surface]");
  story.subscribe((frame) => updateDevicePosters({
    deviceElements,
    frame,
    reviewSurface,
    storyStage,
  }));
  const devices = installDeviceStage(deviceStage, {
    story,
    scenes: STORY_SCENES,
    eventTarget: storyStage,
    onFailure: () => story.useStatic("renderer-failure"),
  });
  window.addEventListener("pagehide", (event) => {
    if (event.persisted) return;
    devices.destroy();
    story.destroy();
  }, { once: true });
}
