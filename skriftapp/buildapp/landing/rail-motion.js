export const VELOCITY_SAMPLE_SCALE = 16;
export const VELOCITY_SMOOTHING = 0.08;
export const VELOCITY_NORMALIZER = 60;
export const MAX_TILT_DEGREES = 16;
export const MAX_SCALE_REDUCTION = 0.03;
export const SWEEP_TRANSLATE_SCALE = 120;
export const SWEEP_OPACITY_SCALE = 0.9;
export const BORDER_ALPHA_BASE = 0.08;
export const BORDER_ALPHA_SCALE = 0.3;
export const BORDER_ACTIVATION_THRESHOLD = 0.05;
export const SETTLE_VELOCITY_THRESHOLD = 0.02;
export const SETTLE_SMOOTHED_THRESHOLD = 0.2;

export function sampleVelocity(scrollDelta, elapsedMilliseconds) {
  return (scrollDelta / Math.max(1, elapsedMilliseconds)) * VELOCITY_SAMPLE_SCALE;
}

export function smoothVelocity(previousSmoothed, sampledVelocity) {
  return previousSmoothed + (sampledVelocity - previousSmoothed) * VELOCITY_SMOOTHING;
}

export function normalizeVelocity(smoothedVelocity) {
  return Math.max(-1, Math.min(1, smoothedVelocity / VELOCITY_NORMALIZER));
}

export function cardTiltTransform(normalizedVelocity, perspectivePixels) {
  const tiltDegrees = (-normalizedVelocity * MAX_TILT_DEGREES).toFixed(2);
  const scale = (1 - Math.abs(normalizedVelocity) * MAX_SCALE_REDUCTION).toFixed(3);
  return `perspective(${perspectivePixels}px) rotateY(${tiltDegrees}deg) scale(${scale})`;
}

export function cardBorderColor(normalizedVelocity, accentChannels) {
  if (Math.abs(normalizedVelocity) <= BORDER_ACTIVATION_THRESHOLD) return "";
  const alpha = (
    BORDER_ALPHA_BASE +
    Math.abs(normalizedVelocity) * BORDER_ALPHA_SCALE
  ).toFixed(2);
  return `rgba(${accentChannels}, ${alpha})`;
}

export function sweepOffsetPercent(normalizedVelocity) {
  return (normalizedVelocity * SWEEP_TRANSLATE_SCALE).toFixed(1);
}

export function sweepOpacity(normalizedVelocity) {
  return (Math.abs(normalizedVelocity) * SWEEP_OPACITY_SCALE).toFixed(2);
}

export function nearestCardIndex(cardCenters, railCenter) {
  return cardCenters.reduce(
    (nearest, center, index) =>
      Math.abs(center - railCenter) < Math.abs(cardCenters[nearest] - railCenter)
        ? index
        : nearest,
    0,
  );
}

export function motionHasSettled(sampledVelocity, smoothedVelocity) {
  return (
    Math.abs(sampledVelocity) <= SETTLE_VELOCITY_THRESHOLD &&
    Math.abs(smoothedVelocity) <= SETTLE_SMOOTHED_THRESHOLD
  );
}
