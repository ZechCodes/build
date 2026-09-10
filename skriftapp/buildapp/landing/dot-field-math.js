export const DOT_BASE_RADIUS = 1.1;
export const DOT_BASE_ALPHA = 0.07;
export const EXCITATION_ALPHA_SCALE = 0.75;
export const EXCITATION_RADIUS_SCALE = 1.6;
export const VIGNETTE_FLOOR = 0.25;
export const VIGNETTE_SCALE = 1.6;
export const VIGNETTE_CENTER_Y_RATIO = 0.48;
export const VIGNETTE_AXIS_RATIO = 0.55;
export const FLAG_MIN_VIGNETTE_DISTANCE = 0.75;
export const FLAG_RIGHT_MARGIN = 110;
export const FLAG_LEFT_MARGIN = 20;
export const FLAG_VERTICAL_MARGIN = 30;

export function vignetteDistance(pixelX, pixelY, fieldWidth, fieldHeight) {
  return Math.hypot(
    (pixelX - fieldWidth / 2) / (fieldWidth * VIGNETTE_AXIS_RATIO),
    (pixelY - fieldHeight * VIGNETTE_CENTER_Y_RATIO) /
      (fieldHeight * VIGNETTE_AXIS_RATIO),
  );
}

export function vignetteFactor(distance) {
  return Math.min(1, Math.max(VIGNETTE_FLOOR, distance * distance * VIGNETTE_SCALE));
}

export function rippleExcitation(pixelX, pixelY, ripple) {
  const ringDistance = Math.hypot(pixelX - ripple.x, pixelY - ripple.y) - ripple.radius;
  if (Math.abs(ringDistance) >= ripple.band) return 0;
  const falloff = 1 - Math.abs(ringDistance) / ripple.band;
  return falloff * falloff * ripple.alpha;
}

export function dotAlpha(excitation, vignette) {
  return (DOT_BASE_ALPHA + excitation * EXCITATION_ALPHA_SCALE) * vignette;
}

export function dotRadius(excitation) {
  return DOT_BASE_RADIUS + excitation * EXCITATION_RADIUS_SCALE;
}

export function isEligibleFlagPosition(pixelX, pixelY, fieldWidth, fieldHeight) {
  return (
    vignetteDistance(pixelX, pixelY, fieldWidth, fieldHeight) >=
      FLAG_MIN_VIGNETTE_DISTANCE &&
    pixelX <= fieldWidth - FLAG_RIGHT_MARGIN &&
    pixelX >= FLAG_LEFT_MARGIN &&
    pixelY >= FLAG_VERTICAL_MARGIN &&
    pixelY <= fieldHeight - FLAG_VERTICAL_MARGIN
  );
}
