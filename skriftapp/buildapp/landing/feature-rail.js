import { readNumericToken, readToken } from "./css-token.js";
import { prefersReducedMotion } from "./motion-preference.js";
import {
  sampleVelocity,
  smoothVelocity,
  normalizeVelocity,
  cardTiltTransform,
  cardBorderColor,
  sweepOffsetPercent,
  sweepOpacity,
  nearestCardIndex,
  motionHasSettled,
} from "./rail-motion.js";

export const ACTIVE_CLASS = "is-active";
export const SWEEP_OFFSET_PROPERTY = "--sweep-offset";
export const SWEEP_OPACITY_PROPERTY = "--sweep-opacity";

const ACCENT_CHANNELS_TOKEN = "--color-accent-channels";
const PERSPECTIVE_TOKEN = "--rail-perspective";
const RESTING_SWEEP_OFFSET = "0";
const RESTING_SWEEP_OPACITY = "0";

export function installFeatureRail({ railElement, cardElements, dashElements }) {
  const accentChannels = readToken(railElement, ACCENT_CHANNELS_TOKEN);
  const perspectivePixels = readNumericToken(railElement, PERSPECTIVE_TOKEN);
  let lastScrollLeft = railElement.scrollLeft;
  let lastTimestamp = 0;
  let smoothedVelocity = 0;
  let isRunning = false;

  function elementCenter(element) {
    const bounds = element.getBoundingClientRect();
    return bounds.left + bounds.width / 2;
  }

  function markActive(activeIndex) {
    cardElements.forEach((cardElement, index) =>
      cardElement.classList.toggle(ACTIVE_CLASS, index === activeIndex),
    );
    dashElements.forEach((dashElement, index) =>
      dashElement.classList.toggle(ACTIVE_CLASS, index === activeIndex),
    );
  }

  function paintMotion(normalizedVelocity) {
    for (const cardElement of cardElements) {
      cardElement.style.transform = cardTiltTransform(
        normalizedVelocity,
        perspectivePixels,
      );
      cardElement.style.setProperty(
        SWEEP_OFFSET_PROPERTY,
        sweepOffsetPercent(normalizedVelocity),
      );
      cardElement.style.setProperty(
        SWEEP_OPACITY_PROPERTY,
        sweepOpacity(normalizedVelocity),
      );
      cardElement.style.borderColor = cardBorderColor(
        normalizedVelocity,
        accentChannels,
      );
    }
  }

  function clearMotion() {
    for (const cardElement of cardElements) {
      cardElement.style.transform = "";
      cardElement.style.borderColor = "";
      cardElement.style.setProperty(SWEEP_OFFSET_PROPERTY, RESTING_SWEEP_OFFSET);
      cardElement.style.setProperty(SWEEP_OPACITY_PROPERTY, RESTING_SWEEP_OPACITY);
    }
  }

  function renderFrame(timestamp) {
    const sampledVelocity = sampleVelocity(
      railElement.scrollLeft - lastScrollLeft,
      timestamp - lastTimestamp,
    );
    lastScrollLeft = railElement.scrollLeft;
    lastTimestamp = timestamp;
    smoothedVelocity = smoothVelocity(smoothedVelocity, sampledVelocity);
    const normalizedVelocity = normalizeVelocity(smoothedVelocity);
    if (!prefersReducedMotion()) paintMotion(normalizedVelocity);
    markActive(nearestCardIndex(cardElements.map(elementCenter), elementCenter(railElement)));
    if (motionHasSettled(sampledVelocity, smoothedVelocity)) {
      isRunning = false;
      smoothedVelocity = 0;
      clearMotion();
      return;
    }
    requestAnimationFrame(renderFrame);
  }

  railElement.addEventListener(
    "scroll",
    () => {
      if (isRunning) return;
      isRunning = true;
      lastTimestamp = performance.now();
      requestAnimationFrame(renderFrame);
    },
    { passive: true },
  );
}
