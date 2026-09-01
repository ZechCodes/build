import { readNumericToken } from "./css-token.js";
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
const FLAG_LABEL_OFFSET_X = 6;
const FLAG_LABEL_OFFSET_Y = 3;
const FLAG_PICK_ATTEMPTS = 200;
const GLOW_EXCITATION_THRESHOLD = 0.55;
const GLOW_BLUR = 8;
const FULL_CIRCLE = Math.PI * 2;
const ACCENT_CHANNELS = "0, 255, 136";
const GLOW_COLOR = "rgba(0, 255, 136, .8)";
const FLAG_DOT_COLOR = "#ffffff";
const NEEDS_YOU_COLOR = "#eafff4";
const NEEDS_YOU_LABEL = "NEEDS YOU";
const NEEDS_YOU_LABEL_FONT = '500 10px "JetBrains Mono"';
const BRACKET_CORNERS = [
  [-1, -1],
  [1, -1],
  [-1, 1],
  [1, 1],
];
const DOT_ALPHA_PRECISION = 3;

export function installDotField(canvasElement) {
  const drawing = canvasElement.getContext("2d");
  let fieldWidth = 0;
  let fieldHeight = 0;
  let gap = 0;
  let columnCount = 0;
  let rowCount = 0;
  let ripples = [];
  let flag = null;
  let nextRippleIn = 0;
  let nextFlagIn = FIRST_FLAG_DELAY;
  let lastTimestamp = 0;

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
    gap = readNumericToken(canvasElement, "--dotfield-gap");
    columnCount = Math.ceil(fieldWidth / gap) + 1;
    rowCount = Math.ceil(fieldHeight / gap) + 1;
  }

  function randomIndex(count) {
    return Math.floor(Math.random() * count);
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
          drawing.shadowColor = GLOW_COLOR;
          drawing.shadowBlur = GLOW_BLUR;
        } else {
          drawing.shadowBlur = 0;
        }
        drawing.beginPath();
        drawing.arc(pixelX, pixelY, dotRadius(excitation), 0, FULL_CIRCLE);
        drawing.fillStyle = `rgba(${ACCENT_CHANNELS}, ${dotAlpha(
          excitation,
          vignette,
        ).toFixed(DOT_ALPHA_PRECISION)})`;
        drawing.fill();
      }
    }
    drawing.shadowBlur = 0;
  }

  function pickFlagPosition() {
    for (let attempt = 0; attempt < FLAG_PICK_ATTEMPTS; attempt += 1) {
      const pixelX = randomIndex(columnCount) * gap;
      const pixelY = randomIndex(rowCount) * gap;
      if (isEligibleFlagPosition(pixelX, pixelY, fieldWidth, fieldHeight)) {
        return { x: pixelX, y: pixelY, age: 0 };
      }
    }
    return null;
  }

  function drawFlagBrackets() {
    drawing.strokeStyle = NEEDS_YOU_COLOR;
    drawing.lineWidth = FLAG_BRACKET_LINE_WIDTH;
    for (const [horizontalSign, verticalSign] of BRACKET_CORNERS) {
      const cornerX = flag.x + horizontalSign * FLAG_BRACKET_RADIUS;
      const cornerY = flag.y + verticalSign * FLAG_BRACKET_RADIUS;
      drawing.beginPath();
      drawing.moveTo(cornerX, cornerY - verticalSign * FLAG_BRACKET_ARM);
      drawing.lineTo(cornerX, cornerY);
      drawing.lineTo(cornerX - horizontalSign * FLAG_BRACKET_ARM, cornerY);
      drawing.stroke();
    }
    drawing.font = NEEDS_YOU_LABEL_FONT;
    drawing.fillStyle = NEEDS_YOU_COLOR;
    drawing.textAlign = "left";
    drawing.fillText(
      NEEDS_YOU_LABEL,
      flag.x + FLAG_BRACKET_RADIUS + FLAG_LABEL_OFFSET_X,
      flag.y + FLAG_LABEL_OFFSET_Y,
    );
  }

  function drawFlag(elapsed) {
    flag = { ...flag, age: flag.age + elapsed };
    if (flag.age % FLAG_BLINK_PERIOD < FLAG_BLINK_ON) drawFlagBrackets();
    drawing.beginPath();
    drawing.arc(flag.x, flag.y, FLAG_DOT_RADIUS, 0, FULL_CIRCLE);
    drawing.fillStyle = FLAG_DOT_COLOR;
    drawing.fill();
    if (flag.age > FLAG_LIFETIME) {
      spawnRipple(flag.x, flag.y, true);
      flag = null;
    }
  }

  function renderFrame(timestamp) {
    const elapsed = Math.min(MAX_FRAME_DELTA, timestamp - lastTimestamp);
    lastTimestamp = timestamp;
    drawing.clearRect(0, 0, fieldWidth, fieldHeight);
    nextRippleIn -= elapsed;
    if (nextRippleIn <= 0) {
      spawnRipple(
        randomIndex(columnCount) * gap,
        randomIndex(rowCount) * gap,
        Math.random() < BIG_RIPPLE_CHANCE,
      );
      nextRippleIn = RIPPLE_INTERVAL_MINIMUM + Math.random() * RIPPLE_INTERVAL_SPREAD;
    }
    ripples = advanceRipples(elapsed);
    drawDots();
    nextFlagIn -= elapsed;
    if (!flag && nextFlagIn <= 0) {
      flag = pickFlagPosition();
      nextFlagIn = FLAG_INTERVAL_MINIMUM + Math.random() * FLAG_INTERVAL_SPREAD;
    }
    if (flag) drawFlag(elapsed);
    if (!prefersReducedMotion()) requestAnimationFrame(renderFrame);
  }

  resizeField();
  addEventListener("resize", resizeField);
  requestAnimationFrame(renderFrame);
}
