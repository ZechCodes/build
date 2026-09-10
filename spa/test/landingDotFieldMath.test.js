import { describe, expect, it } from "vitest";
import {
  DOT_BASE_RADIUS,
  DOT_BASE_ALPHA,
  EXCITATION_ALPHA_SCALE,
  EXCITATION_RADIUS_SCALE,
  VIGNETTE_FLOOR,
  FLAG_MIN_VIGNETTE_DISTANCE,
  FLAG_RIGHT_MARGIN,
  FLAG_LEFT_MARGIN,
  FLAG_VERTICAL_MARGIN,
  vignetteDistance,
  vignetteFactor,
  rippleExcitation,
  dotAlpha,
  dotRadius,
  isEligibleFlagPosition,
} from "../../skriftapp/buildapp/landing/dot-field-math.js";

const FIELD_WIDTH = 1000;
const FIELD_HEIGHT = 1000;
const FOCUS_X = FIELD_WIDTH / 2;
const FOCUS_Y = FIELD_HEIGHT * 0.48;

describe("dot field math", () => {
  it("vignette distance is zero at the focus point", () => {
    expect(vignetteDistance(FOCUS_X, FOCUS_Y, FIELD_WIDTH, FIELD_HEIGHT)).toBe(0);
  });

  it("vignette factor is floored at a quarter near the focus point", () => {
    expect(VIGNETTE_FLOOR).toBe(0.25);
    expect(vignetteFactor(0)).toBe(0.25);
    expect(
      vignetteFactor(vignetteDistance(FOCUS_X, FOCUS_Y + 10, FIELD_WIDTH, FIELD_HEIGHT)),
    ).toBe(0.25);
  });

  it("vignette factor reaches full strength at the field corners", () => {
    expect(vignetteFactor(vignetteDistance(0, 0, FIELD_WIDTH, FIELD_HEIGHT))).toBe(1);
    expect(
      vignetteFactor(
        vignetteDistance(FIELD_WIDTH, FIELD_HEIGHT, FIELD_WIDTH, FIELD_HEIGHT),
      ),
    ).toBe(1);
  });

  it("ripple excitation is zero outside the band", () => {
    const ripple = { x: 0, y: 0, radius: 100, band: 70, alpha: 1 };
    expect(rippleExcitation(300, 0, ripple)).toBe(0);
    expect(rippleExcitation(170, 0, ripple)).toBe(0);
    expect(rippleExcitation(0, 0, ripple)).toBe(0);
  });

  it("ripple excitation peaks on the ring and falls off quadratically", () => {
    const ripple = { x: 0, y: 0, radius: 100, band: 70, alpha: 0.5 };
    expect(rippleExcitation(100, 0, ripple)).toBeCloseTo(0.5);
    expect(rippleExcitation(135, 0, ripple)).toBeCloseTo(0.125);
    expect(rippleExcitation(65, 0, ripple)).toBeCloseTo(0.125);
  });

  it("dot alpha and radius grow with excitation from the documented base", () => {
    expect(DOT_BASE_ALPHA).toBe(0.07);
    expect(DOT_BASE_RADIUS).toBe(1.1);
    expect(EXCITATION_ALPHA_SCALE).toBe(0.75);
    expect(EXCITATION_RADIUS_SCALE).toBe(1.6);
    expect(dotAlpha(0, 1)).toBeCloseTo(0.07);
    expect(dotAlpha(1, 1)).toBeCloseTo(0.82);
    expect(dotAlpha(1, 0.5)).toBeCloseTo(0.41);
    expect(dotRadius(0)).toBeCloseTo(1.1);
    expect(dotRadius(1)).toBeCloseTo(2.7);
  });

  it("flag positions near the focus point are rejected", () => {
    expect(FLAG_MIN_VIGNETTE_DISTANCE).toBe(0.75);
    expect(isEligibleFlagPosition(FOCUS_X, FOCUS_Y, FIELD_WIDTH, FIELD_HEIGHT)).toBe(
      false,
    );
    expect(
      isEligibleFlagPosition(FOCUS_X + 100, FOCUS_Y, FIELD_WIDTH, FIELD_HEIGHT),
    ).toBe(false);
  });

  it("flag positions inside the edge margins are rejected", () => {
    expect(FLAG_RIGHT_MARGIN).toBe(110);
    expect(FLAG_LEFT_MARGIN).toBe(20);
    expect(FLAG_VERTICAL_MARGIN).toBe(30);
    expect(isEligibleFlagPosition(950, 100, FIELD_WIDTH, FIELD_HEIGHT)).toBe(false);
    expect(isEligibleFlagPosition(10, 100, FIELD_WIDTH, FIELD_HEIGHT)).toBe(false);
    expect(isEligibleFlagPosition(100, 10, FIELD_WIDTH, FIELD_HEIGHT)).toBe(false);
    expect(isEligibleFlagPosition(100, 990, FIELD_WIDTH, FIELD_HEIGHT)).toBe(false);
  });

  it("a far corner position clear of the margins is accepted", () => {
    expect(isEligibleFlagPosition(100, 100, FIELD_WIDTH, FIELD_HEIGHT)).toBe(true);
    expect(isEligibleFlagPosition(880, 940, FIELD_WIDTH, FIELD_HEIGHT)).toBe(true);
  });
});
