import { installNavSolidityObserver } from "./nav-solidity.js";
import { installMobileMenu } from "./mobile-menu.js";
import { installDotField } from "./dot-field.js";
import { installFeatureRail } from "./feature-rail.js";
import { installWaitlist } from "./waitlist-form.js";
import { submitWaitlistEmail } from "./waitlist-api.js";
import { installCinematicStory } from "./cinematic-story.js";
import { installStoryDevices } from "./story-devices.js";

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
  const devices = installStoryDevices({ story, storyStage, deviceStage });
  window.addEventListener("pagehide", (event) => {
    if (event.persisted) return;
    devices.destroy();
    story.destroy();
  }, { once: true });
}
