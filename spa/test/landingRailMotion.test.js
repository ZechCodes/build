import { describe, expect, it } from "vitest";
import {
  VELOCITY_SAMPLE_SCALE,
  VELOCITY_SMOOTHING,
  VELOCITY_NORMALIZER,
  MAX_TILT_DEGREES,
  MAX_SCALE_REDUCTION,
  SWEEP_TRANSLATE_SCALE,
  SWEEP_OPACITY_SCALE,
  BORDER_ALPHA_BASE,
  BORDER_ALPHA_SCALE,
  BORDER_ACTIVATION_THRESHOLD,
  SETTLE_VELOCITY_THRESHOLD,
  SETTLE_SMOOTHED_THRESHOLD,
  ACCENT_CHANNELS,
  sampleVelocity,
  smoothVelocity,
  normalizeVelocity,
  cardTiltTransform,
  cardBorderColor,
  sweepOffsetPercent,
  sweepOpacity,
  nearestCardIndex,
  motionHasSettled,
} from "../../skriftapp/buildapp/landing/rail-motion.js";

describe("rail motion math", () => {
  it("sampled velocity scales scroll distance per frame and never divides by less than one millisecond", () => {
    expect(VELOCITY_SAMPLE_SCALE).toBe(16);
    expect(sampleVelocity(32, 16)).toBeCloseTo(32);
    expect(sampleVelocity(-32, 16)).toBeCloseTo(-32);
    expect(sampleVelocity(10, 0)).toBeCloseTo(160);
    expect(sampleVelocity(10, 0.25)).toBeCloseTo(160);
  });

  it("smoothing moves the running value toward the sample by eight percent", () => {
    expect(VELOCITY_SMOOTHING).toBe(0.08);
    expect(smoothVelocity(0, 100)).toBeCloseTo(8);
    expect(smoothVelocity(10, 10)).toBeCloseTo(10);
    expect(smoothVelocity(100, 0)).toBeCloseTo(92);
  });

  it("normalized velocity clamps to plus and minus one", () => {
    expect(VELOCITY_NORMALIZER).toBe(60);
    expect(normalizeVelocity(30)).toBeCloseTo(0.5);
    expect(normalizeVelocity(600)).toBe(1);
    expect(normalizeVelocity(-600)).toBe(-1);
  });

  it("tilt rotates opposite the scroll direction", () => {
    expect(MAX_TILT_DEGREES).toBe(16);
    expect(cardTiltTransform(0.5)).toContain("rotateY(-8.00deg)");
    expect(cardTiltTransform(-0.5)).toContain("rotateY(8.00deg)");
    expect(cardTiltTransform(1)).toContain("perspective(900px)");
  });

  it("scale shrinks with speed and returns to one at rest", () => {
    expect(MAX_SCALE_REDUCTION).toBe(0.03);
    expect(cardTiltTransform(1)).toContain("scale(0.970)");
    expect(cardTiltTransform(-1)).toContain("scale(0.970)");
    expect(cardTiltTransform(0)).toBe(
      "perspective(900px) rotateY(0.00deg) scale(1.000)",
    );
  });

  it("border colour is empty at or below the activation threshold", () => {
    expect(BORDER_ACTIVATION_THRESHOLD).toBe(0.05);
    expect(cardBorderColor(0)).toBe("");
    expect(cardBorderColor(0.05)).toBe("");
    expect(cardBorderColor(-0.05)).toBe("");
  });

  it("border alpha grows from the base with speed", () => {
    expect(BORDER_ALPHA_BASE).toBe(0.08);
    expect(BORDER_ALPHA_SCALE).toBe(0.3);
    expect(cardBorderColor(0.5)).toBe(`rgba(${ACCENT_CHANNELS}, 0.23)`);
    expect(cardBorderColor(-0.5)).toBe(`rgba(${ACCENT_CHANNELS}, 0.23)`);
    expect(cardBorderColor(1)).toBe(`rgba(${ACCENT_CHANNELS}, 0.38)`);
  });

  it("sweep offset and opacity track the signed and absolute velocity", () => {
    expect(SWEEP_TRANSLATE_SCALE).toBe(120);
    expect(SWEEP_OPACITY_SCALE).toBe(0.9);
    expect(sweepOffsetPercent(0.5)).toBe("60.0");
    expect(sweepOffsetPercent(-0.5)).toBe("-60.0");
    expect(sweepOpacity(0.5)).toBe("0.45");
    expect(sweepOpacity(-0.5)).toBe("0.45");
  });

  it("nearest card index picks the card closest to the rail centre and breaks ties low", () => {
    expect(nearestCardIndex([10, 50, 90], 48)).toBe(1);
    expect(nearestCardIndex([10, 50, 90], 95)).toBe(2);
    expect(nearestCardIndex([0, 100], 50)).toBe(0);
  });

  it("motion settles only when both velocity terms fall below their thresholds", () => {
    expect(SETTLE_VELOCITY_THRESHOLD).toBe(0.02);
    expect(SETTLE_SMOOTHED_THRESHOLD).toBe(0.2);
    expect(motionHasSettled(0.01, 0.1)).toBe(true);
    expect(motionHasSettled(0.03, 0.1)).toBe(false);
    expect(motionHasSettled(0.01, 0.5)).toBe(false);
    expect(motionHasSettled(-0.01, -0.1)).toBe(true);
  });
});
