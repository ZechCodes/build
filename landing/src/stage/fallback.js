// Which version of the story a visitor gets. Pure: the page measures the
// signals, this decides. A slow renderer is a reason to stop drawing 3D, never
// a reason to take the story apart.
export const STAGE_LIMITS = Object.freeze({
  // Under this width the pinned film cannot hold a headline, a device and a
  // demonstration card at once, so phones read the story as a document.
  documentMaxWidth: 768,
  // A frame costing more than this leaves nothing for scrolling: hand back to
  // the posters.
  slowFrameMs: 34,
  reduceQualityFrameMs: 20,
  reducedQualityScale: 0.65,
});

function slowFrame(frameCostMs, limit) {
  return Number.isFinite(frameCostMs) && frameCostMs > limit;
}

export function stageModeReason(signals = {}) {
  const { reducedMotion, viewportWidth, saveData, webgl, frameCostMs } = signals;
  if (reducedMotion) return "reduced-motion";
  if (!webgl) return "no-webgl";
  if (!(Number(viewportWidth) >= STAGE_LIMITS.documentMaxWidth)) return "viewport";
  if (saveData) return "save-data";
  if (slowFrame(frameCostMs, STAGE_LIMITS.slowFrameMs)) return "frame-budget";
  return null;
}

export function stageMode(signals) {
  return stageModeReason(signals) === null ? "stage" : "document";
}

// Between the two limits the stage keeps drawing at a lower pixel ratio, the
// same ratchet the old stage used before giving up.
export function renderQualityScale(frameCostMs) {
  return slowFrame(frameCostMs, STAGE_LIMITS.reduceQualityFrameMs)
    ? STAGE_LIMITS.reducedQualityScale
    : 1;
}
