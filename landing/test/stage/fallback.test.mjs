import assert from "node:assert/strict";
import test from "node:test";
import {
  STAGE_LIMITS,
  renderQualityScale,
  stageMode,
  stageModeReason,
} from "../../src/stage/fallback.js";

const capable = Object.freeze({
  reducedMotion: false,
  viewportWidth: 1440,
  saveData: false,
  webgl: true,
  frameCostMs: 9,
});

test("a capable viewport runs the stage", () => {
  assert.equal(stageMode(capable), "stage");
  assert.equal(stageModeReason(capable), null);
  assert.equal(stageMode({ ...capable, frameCostMs: undefined }), "stage", "unmeasured is not slow");
  assert.equal(stageMode({ ...capable, viewportWidth: STAGE_LIMITS.documentMaxWidth }), "stage");
});

test("each fallback signal sends the page to the document version", () => {
  const cases = [
    [{ reducedMotion: true }, "reduced-motion"],
    [{ viewportWidth: 767 }, "viewport"],
    [{ viewportWidth: 390 }, "viewport"],
    [{ saveData: true }, "save-data"],
    [{ webgl: false }, "no-webgl"],
    [{ frameCostMs: 40 }, "frame-budget"],
  ];
  for (const [signal, reason] of cases) {
    const signals = { ...capable, ...signal };
    assert.equal(stageModeReason(signals), reason);
    assert.equal(stageMode(signals), "document");
  }
});

test("reduced motion wins over a fast machine and missing signals fall back safely", () => {
  assert.equal(stageMode({ ...capable, reducedMotion: true, frameCostMs: 1 }), "document");
  assert.equal(stageMode({}), "document", "no viewport and no webgl is the document");
  assert.equal(stageModeReason({}), "no-webgl");
});

test("a frame under the budget keeps full resolution, a heavy one drops it", () => {
  assert.equal(renderQualityScale(9), 1);
  assert.equal(renderQualityScale(STAGE_LIMITS.reduceQualityFrameMs + 1), STAGE_LIMITS.reducedQualityScale);
  assert.equal(renderQualityScale(undefined), 1);
  assert(STAGE_LIMITS.reduceQualityFrameMs < STAGE_LIMITS.slowFrameMs);
  assert.equal(STAGE_LIMITS.slowFrameMs, 34);
  assert.equal(STAGE_LIMITS.documentMaxWidth, 768);
});
