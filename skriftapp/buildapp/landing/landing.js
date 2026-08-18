// Animated backdrop: a perspective-projected floor of neon dots rolling a slow
// wave toward the horizon, mirrored faintly overhead. Pure canvas, no deps.
// Honors prefers-reduced-motion (single static frame) and pauses when hidden.
(() => {
  "use strict";

  const canvas = document.getElementById("grid");
  const context = canvas.getContext("2d");
  const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;

  const GRID_COLUMNS = 44;
  const GRID_ROWS = 26;
  const NEON = { red: 0, green: 255, blue: 156 };

  let width = 0;
  let height = 0;

  function resize() {
    const pixelRatio = Math.min(devicePixelRatio || 1, 2);
    width = innerWidth;
    height = innerHeight;
    canvas.width = width * pixelRatio;
    canvas.height = height * pixelRatio;
    context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
  }

  // Project a grid cell onto the canvas. rowDepth 0 is nearest the viewer,
  // 1 is the horizon; the wave displaces dot height before projection.
  function projectDot(column, rowDepth, waveOffset, horizonY, mirrored) {
    const depth = 0.08 + rowDepth * 0.92;
    const spread = (column / (GRID_COLUMNS - 1) - 0.5) * 2;
    const x = width / 2 + spread * (width * 0.85) * (1 - rowDepth * 0.72);
    const floorSpan = mirrored ? -height * 0.42 : height * 0.62;
    const y = horizonY + floorSpan * (1 - rowDepth) ** 1.6 + waveOffset * (1 - rowDepth);
    return { x, y, depth };
  }

  function drawPlane(time, horizonY, mirrored) {
    const baseAlpha = mirrored ? 0.22 : 1;
    for (let row = 0; row < GRID_ROWS; row++) {
      const rowDepth = row / (GRID_ROWS - 1);
      for (let column = 0; column < GRID_COLUMNS; column++) {
        const phase = column * 0.35 + row * 0.55;
        const wave = Math.sin(time * 0.0009 + phase) * 14 * (mirrored ? -1 : 1);
        const { x, y, depth } = projectDot(column, rowDepth, wave, horizonY, mirrored);
        const fade = (1 - rowDepth) ** 1.35;
        const shimmer = 0.55 + 0.45 * Math.sin(time * 0.0013 + phase * 1.7);
        const alpha = baseAlpha * fade * (0.14 + 0.3 * shimmer);
        if (alpha < 0.01) continue;
        const radius = Math.max(0.6, 2.4 * (1 - depth * 0.75));
        context.fillStyle = `rgba(${NEON.red},${NEON.green},${NEON.blue},${alpha.toFixed(3)})`;
        context.beginPath();
        context.arc(x, y, radius, 0, Math.PI * 2);
        context.fill();
      }
    }
  }

  function drawFrame(time) {
    context.clearRect(0, 0, width, height);
    const horizonY = height * 0.38;

    // glow bloom at the horizon line
    const bloom = context.createRadialGradient(
      width / 2, horizonY, 0,
      width / 2, horizonY, Math.max(width, height) * 0.55,
    );
    bloom.addColorStop(0, "rgba(0,255,156,0.055)");
    bloom.addColorStop(1, "rgba(0,255,156,0)");
    context.fillStyle = bloom;
    context.fillRect(0, 0, width, height);

    drawPlane(time, horizonY, false);
    drawPlane(time, horizonY, true);
  }

  let animationFrame = 0;
  function loop(time) {
    drawFrame(time);
    animationFrame = requestAnimationFrame(loop);
  }

  addEventListener("resize", () => {
    resize();
    if (reducedMotion) drawFrame(0);
  });

  document.addEventListener("visibilitychange", () => {
    if (reducedMotion) return;
    if (document.hidden) {
      cancelAnimationFrame(animationFrame);
    } else {
      animationFrame = requestAnimationFrame(loop);
    }
  });

  resize();
  if (reducedMotion) {
    drawFrame(0);
  } else {
    animationFrame = requestAnimationFrame(loop);
  }
})();
