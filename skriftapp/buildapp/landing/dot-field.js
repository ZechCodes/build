import { readNumericToken, readToken } from "./css-token.js";
import { prefersReducedMotion } from "./motion-preference.js";
import {
  vignetteDistance,
  vignetteFactor,
  rippleExcitation,
  dotAlpha,
  dotRadius,
  isEligibleFlagPosition,
} from "./dot-field-math.js";

const MAX_DEVICE_PIXEL_RATIO = 2;
const MAX_FRAME_DELTA = 60;
const RIPPLE_SPEED = 0.09;
const BIG_RIPPLE_SPEED = 0.13;
const RIPPLE_BAND = 70;
const BIG_RIPPLE_BAND = 110;
const BIG_RIPPLE_CHANCE = 0.3;
const RIPPLE_INTERVAL_MINIMUM = 1500;
const RIPPLE_INTERVAL_SPREAD = 1800;
const RIPPLE_FADE_RATIO = 0.9;
const FIRST_FLAG_DELAY = 1800;
const FLAG_INTERVAL_MINIMUM = 5500;
const FLAG_INTERVAL_SPREAD = 3000;
const FLAG_LIFETIME = 5200;
const FLAG_BLINK_PERIOD = 900;
const FLAG_BLINK_ON = 650;
const FLAG_BRACKET_RADIUS = 10;
const FLAG_BRACKET_ARM = 5;
const FLAG_BRACKET_LINE_WIDTH = 1.5;
const FLAG_DOT_RADIUS = 2.6;
const FLAG_DOT_COLOR = "#ffffff";
const FLAG_LABEL_OFFSET_X = 6;
const FLAG_LABEL_OFFSET_Y = 3;
const FLAG_LABEL_WEIGHT = 500;
const FLAG_LABEL_SIZE = 10;
const FLAG_LABEL = "NEEDS YOU";
const FLAG_PICK_ATTEMPTS = 200;
const GLOW_EXCITATION_THRESHOLD = 0.55;
const GLOW_ALPHA = 0.8;
const GLOW_BLUR = 8;
const FULL_CIRCLE = Math.PI * 2;
const DOT_ALPHA_PRECISION = 3;
const ACCENT_CHANNELS_TOKEN = "--color-accent-channels";
const FLAG_COLOR_TOKEN = "--color-alert";
const FONT_FAMILY_TOKEN = "--font-mono";
const DOTFIELD_GAP_TOKEN = "--dotfield-gap";
const BRACKET_CORNERS = [
  [-1, -1],
  [1, -1],
  [-1, 1],
  [1, 1],
];

export function installDotField(canvasElement) {
  const drawing = canvasElement.getContext("2d");
  let fieldWidth = 0;
  let fieldHeight = 0;
  let gap = 0;
  let columnCount = 0;
  let rowCount = 0;
  let dotColorPrefix = "";
  let glowColor = "";
  let flagColor = "";
  let flagLabelFont = "";
  let ripples = [];
  let flag = null;
  let nextRippleIn = 0;
  let nextFlagIn = FIRST_FLAG_DELAY;
  let lastTimestamp = 0;
  let isAnimating = true;

  function readPalette() {
    const accentChannels = readToken(canvasElement, ACCENT_CHANNELS_TOKEN);
    dotColorPrefix = `rgba(${accentChannels}, `;
    glowColor = `rgba(${accentChannels}, ${GLOW_ALPHA})`;
    flagColor = readToken(canvasElement, FLAG_COLOR_TOKEN);
    flagLabelFont = `${FLAG_LABEL_WEIGHT} ${FLAG_LABEL_SIZE}px ${readToken(
      canvasElement,
      FONT_FAMILY_TOKEN,
    )}`;
  }

  function resizeField() {
    const pixelRatio = Math.min(MAX_DEVICE_PIXEL_RATIO, devicePixelRatio || 1);
    const bounds = canvasElement.parentElement.getBoundingClientRect();
    fieldWidth = bounds.width;
    fieldHeight = bounds.height;
    canvasElement.width = fieldWidth * pixelRatio;
    canvasElement.height = fieldHeight * pixelRatio;
    canvasElement.style.width = `${fieldWidth}px`;
    canvasElement.style.height = `${fieldHeight}px`;
    drawing.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
    gap = readNumericToken(canvasElement, DOTFIELD_GAP_TOKEN);
    columnCount = Math.ceil(fieldWidth / gap) + 1;
    rowCount = Math.ceil(fieldHeight / gap) + 1;
    readPalette();
  }

  function randomGridPoint() {
    return {
      x: Math.floor(Math.random() * columnCount) * gap,
      y: Math.floor(Math.random() * rowCount) * gap,
    };
  }

  function spawnRipple(originX, originY, isBig) {
    ripples = ripples.concat({
      x: originX,
      y: originY,
      radius: 0,
      speed: isBig ? BIG_RIPPLE_SPEED : RIPPLE_SPEED,
      band: isBig ? BIG_RIPPLE_BAND : RIPPLE_BAND,
      alpha: 1,
    });
  }

  function advanceRipples(elapsed) {
    const fadeDistance = Math.max(fieldWidth, fieldHeight) * RIPPLE_FADE_RATIO;
    return ripples
      .map((ripple) => {
        const radius = ripple.radius + ripple.speed * elapsed;
        return { ...ripple, radius, alpha: Math.max(0, 1 - radius / fadeDistance) };
      })
      .filter((ripple) => ripple.alpha > 0);
  }

  function advanceRippleSchedule(elapsed) {
    nextRippleIn -= elapsed;
    if (nextRippleIn > 0) return;
    const origin = randomGridPoint();
    spawnRipple(origin.x, origin.y, Math.random() < BIG_RIPPLE_CHANCE);
    nextRippleIn = RIPPLE_INTERVAL_MINIMUM + Math.random() * RIPPLE_INTERVAL_SPREAD;
  }

  function advanceFlagSchedule(elapsed) {
    nextFlagIn -= elapsed;
    if (flag || nextFlagIn > 0) return;
    flag = pickFlagPosition();
    nextFlagIn = FLAG_INTERVAL_MINIMUM + Math.random() * FLAG_INTERVAL_SPREAD;
  }

  function excitationAt(pixelX, pixelY) {
    return ripples.reduce(
      (strongest, ripple) =>
        Math.max(strongest, rippleExcitation(pixelX, pixelY, ripple)),
      0,
    );
  }

  function drawDots() {
    for (let columnIndex = 0; columnIndex < columnCount; columnIndex += 1) {
      for (let rowIndex = 0; rowIndex < rowCount; rowIndex += 1) {
        const pixelX = columnIndex * gap;
        const pixelY = rowIndex * gap;
        const excitation = excitationAt(pixelX, pixelY);
        const vignette = vignetteFactor(
          vignetteDistance(pixelX, pixelY, fieldWidth, fieldHeight),
        );
        if (excitation > GLOW_EXCITATION_THRESHOLD) {
          drawing.shadowColor = glowColor;
          drawing.shadowBlur = GLOW_BLUR;
        } else {
          drawing.shadowBlur = 0;
        }
        drawing.beginPath();
        drawing.arc(pixelX, pixelY, dotRadius(excitation), 0, FULL_CIRCLE);
        drawing.fillStyle = `${dotColorPrefix}${dotAlpha(excitation, vignette).toFixed(
          DOT_ALPHA_PRECISION,
        )})`;
        drawing.fill();
      }
    }
    drawing.shadowBlur = 0;
  }

  function pickFlagPosition() {
    for (let attempt = 0; attempt < FLAG_PICK_ATTEMPTS; attempt += 1) {
      const candidate = randomGridPoint();
      if (isEligibleFlagPosition(candidate.x, candidate.y, fieldWidth, fieldHeight)) {
        return { x: candidate.x, y: candidate.y, age: 0 };
      }
    }
    return null;
  }

  function drawFlagBrackets(currentFlag) {
    drawing.strokeStyle = flagColor;
    drawing.lineWidth = FLAG_BRACKET_LINE_WIDTH;
    for (const [horizontalSign, verticalSign] of BRACKET_CORNERS) {
      const cornerX = currentFlag.x + horizontalSign * FLAG_BRACKET_RADIUS;
      const cornerY = currentFlag.y + verticalSign * FLAG_BRACKET_RADIUS;
      drawing.beginPath();
      drawing.moveTo(cornerX, cornerY - verticalSign * FLAG_BRACKET_ARM);
      drawing.lineTo(cornerX, cornerY);
      drawing.lineTo(cornerX - horizontalSign * FLAG_BRACKET_ARM, cornerY);
      drawing.stroke();
    }
    drawing.font = flagLabelFont;
    drawing.fillStyle = flagColor;
    drawing.textAlign = "left";
    drawing.fillText(
      FLAG_LABEL,
      currentFlag.x + FLAG_BRACKET_RADIUS + FLAG_LABEL_OFFSET_X,
      currentFlag.y + FLAG_LABEL_OFFSET_Y,
    );
  }

  function drawFlag(currentFlag) {
    if (currentFlag.age % FLAG_BLINK_PERIOD < FLAG_BLINK_ON) {
      drawFlagBrackets(currentFlag);
    }
    drawing.beginPath();
    drawing.arc(currentFlag.x, currentFlag.y, FLAG_DOT_RADIUS, 0, FULL_CIRCLE);
    drawing.fillStyle = FLAG_DOT_COLOR;
    drawing.fill();
  }

  function renderFrame(timestamp) {
    const elapsed = Math.min(MAX_FRAME_DELTA, timestamp - lastTimestamp);
    lastTimestamp = timestamp;
    drawing.clearRect(0, 0, fieldWidth, fieldHeight);
    advanceRippleSchedule(elapsed);
    ripples = advanceRipples(elapsed);
    drawDots();
    advanceFlagSchedule(elapsed);
    if (flag) {
      flag = { ...flag, age: flag.age + elapsed };
      drawFlag(flag);
      if (flag.age > FLAG_LIFETIME) {
        spawnRipple(flag.x, flag.y, true);
        flag = null;
      }
    }
    isAnimating = !prefersReducedMotion();
    if (isAnimating) requestAnimationFrame(renderFrame);
  }

  resizeField();
  addEventListener("resize", () => {
    resizeField();
    if (isAnimating) return;
    isAnimating = true;
    requestAnimationFrame(renderFrame);
  });
  requestAnimationFrame(renderFrame);
}
