// The animation state behind the rail's canvas patterns, with nothing drawn.
// Every value a frame needs is derived here so the painter stays a painter, and
// so the two properties that matter — the same agent always moves the same way,
// and a paused bubble holds its frame — are testable without a canvas.

import { describe, it, expect } from "vitest";
import {
  mulberry32,
  hashString,
  motionParams,
  createClock,
  cellPhase,
  CELL_SCALE_BASE,
  CELL_ALPHA_BASE,
} from "../src/core/patternMotion.js";

const drawSequence = (random, count) => Array.from({ length: count }, () => random());

describe("mulberry32", () => {
  it("replays the same sequence for the same seed", () => {
    expect(drawSequence(mulberry32(12345), 8)).toEqual(drawSequence(mulberry32(12345), 8));
  });

  it("draws a different sequence for a different seed", () => {
    expect(drawSequence(mulberry32(1), 8)).not.toEqual(drawSequence(mulberry32(2), 8));
  });

  it("stays inside [0, 1)", () => {
    for (const seed of [0, 1, 7, 99991, 0xffffffff]) {
      for (const value of drawSequence(mulberry32(seed), 200)) {
        expect(value).toBeGreaterThanOrEqual(0);
        expect(value).toBeLessThan(1);
      }
    }
  });

  it("keeps advancing rather than settling on one value", () => {
    const [first, second, third] = drawSequence(mulberry32(0), 3);
    expect(new Set([first, second, third]).size).toBe(3);
  });
});

describe("hashString", () => {
  it("is a stable unsigned 32-bit integer", () => {
    const hash = hashString("agent-7");
    expect(hash).toBe(hashString("agent-7"));
    expect(Number.isInteger(hash)).toBe(true);
    expect(hash).toBeGreaterThanOrEqual(0);
    expect(hash).toBeLessThan(2 ** 32);
  });

  it("separates ids that differ by one character, and by order", () => {
    expect(hashString("agent-7")).not.toBe(hashString("agent-8"));
    expect(hashString("ab")).not.toBe(hashString("ba"));
  });

  it("hashes the empty string and a missing id without throwing", () => {
    expect(Number.isInteger(hashString(""))).toBe(true);
    expect(Number.isInteger(hashString(null))).toBe(true);
    expect(Number.isInteger(hashString(undefined))).toBe(true);
  });
});

describe("motionParams", () => {
  const seeds = Array.from({ length: 300 }, (_, index) => index * 7919 + 13);

  it("hands back identical params for the same seed", () => {
    expect(motionParams(42)).toEqual(motionParams(42));
  });

  it("moves a different agent differently", () => {
    expect(motionParams(42)).not.toEqual(motionParams(43));
  });

  it("takes an agent id directly, hashing it the same way a caller would", () => {
    expect(motionParams("agent-7")).toEqual(motionParams(hashString("agent-7")));
  });

  it("drifts along a unit vector", () => {
    for (const seed of seeds) {
      const { drift } = motionParams(seed);
      expect(Math.hypot(drift.x, drift.y)).toBeCloseTo(1, 10);
    }
  });

  it("crosses one lattice cell in 4 to 8 seconds, so travel tiles seamlessly", () => {
    for (const seed of seeds) {
      const { driftSecondsPerCell } = motionParams(seed);
      expect(driftSecondsPerCell).toBeGreaterThanOrEqual(4);
      expect(driftSecondsPerCell).toBeLessThanOrEqual(8);
    }
  });

  it("rotates slowly in either direction, and sometimes barely at all", () => {
    const speeds = seeds.map((seed) => motionParams(seed).rotationSpeed);
    for (const speed of speeds) {
      expect(speed).toBeGreaterThanOrEqual(-0.03);
      expect(speed).toBeLessThanOrEqual(0.03);
    }
    expect(speeds.some((speed) => speed > 0)).toBe(true);
    expect(speeds.some((speed) => speed < 0)).toBe(true);
    expect(speeds.some((speed) => Math.abs(speed) < 0.005)).toBe(true);
  });

  it("gives the wave a real direction, so the ripple crosses the lattice", () => {
    for (const seed of seeds) {
      const { wave, waveFrequency } = motionParams(seed);
      expect(Math.hypot(wave.kx, wave.ky)).toBeGreaterThan(0.2);
      expect(Math.hypot(wave.kx, wave.ky)).toBeLessThanOrEqual(1.2);
      expect(waveFrequency).toBeGreaterThan(0);
      expect(waveFrequency).toBeLessThanOrEqual(1);
    }
  });

  it("keeps the breathing subtle: scale within 4-12%, alpha within 0.15-0.35", () => {
    for (const seed of seeds) {
      const { scaleAmplitude, alphaAmplitude } = motionParams(seed);
      expect(scaleAmplitude).toBeGreaterThanOrEqual(0.04);
      expect(scaleAmplitude).toBeLessThanOrEqual(0.12);
      expect(alphaAmplitude).toBeGreaterThanOrEqual(0.15);
      expect(alphaAmplitude).toBeLessThanOrEqual(0.35);
    }
  });

  it("starts each agent at its own point in the loop", () => {
    const phases = seeds.map((seed) => motionParams(seed).phase);
    for (const phase of phases) {
      expect(phase).toBeGreaterThanOrEqual(0);
      expect(phase).toBeLessThan(Math.PI * 2);
    }
    expect(new Set(phases).size).toBeGreaterThan(seeds.length / 2);
  });
});

describe("createClock", () => {
  it("reads zero before anything has happened", () => {
    expect(createClock().now()).toBe(0);
  });

  it("does not move while stopped — this is what holds a paused bubble's frame", () => {
    const clock = createClock();
    clock.advance(500);
    clock.advance(500);
    expect(clock.now()).toBe(0);
  });

  it("accumulates running time in seconds", () => {
    const clock = createClock();
    clock.start();
    clock.advance(250);
    clock.advance(750);
    expect(clock.now()).toBeCloseTo(1, 10);
  });

  it("resumes from the held value rather than restarting", () => {
    const clock = createClock();
    clock.start();
    clock.advance(1000);
    clock.stop();
    clock.advance(5000);
    expect(clock.now()).toBeCloseTo(1, 10);
    clock.start();
    clock.advance(500);
    expect(clock.now()).toBeCloseTo(1.5, 10);
  });

  it("ignores a start while already running, and a stop while already stopped", () => {
    const clock = createClock();
    clock.start();
    clock.start();
    clock.advance(1000);
    clock.stop();
    clock.stop();
    expect(clock.now()).toBeCloseTo(1, 10);
  });

  it("never runs backwards on a garbage delta", () => {
    const clock = createClock();
    clock.start();
    clock.advance(1000);
    clock.advance(-5000);
    clock.advance(NaN);
    clock.advance(undefined);
    expect(clock.now()).toBeCloseTo(1, 10);
  });
});

describe("cellPhase", () => {
  const params = {
    waveFrequency: 2,
    wave: { kx: 0.5, ky: 0.25 },
    phase: 0,
    scaleAmplitude: 0.1,
    alphaAmplitude: 0.2,
  };

  it("is the wave sampled at the cell", () => {
    // angle = 2*0.5 + 0.5*1 + 0.25*2 = 2
    const { scale, alpha } = cellPhase(params, { col: 1, row: 2 }, 0.5);
    expect(scale).toBeCloseTo(CELL_SCALE_BASE + 0.1 * Math.sin(2), 12);
    expect(alpha).toBeCloseTo(CELL_ALPHA_BASE + 0.2 * Math.sin(2), 12);
  });

  it("carries the global phase offset into the angle", () => {
    const offset = { ...params, phase: Math.PI / 3 };
    const { scale } = cellPhase(offset, { col: 0, row: 0 }, 0);
    expect(scale).toBeCloseTo(CELL_SCALE_BASE + 0.1 * Math.sin(Math.PI / 3), 12);
  });

  it("separates neighbours by the spatial phase term alone — a ripple, not jitter", () => {
    const here = cellPhase(params, { col: 3, row: 4 }, 1.25);
    // One column over is this cell kx/omega seconds later: the same motion,
    // arriving late. Independent jitter could not be written as a time shift.
    const rightNeighbour = cellPhase(params, { col: 4, row: 4 }, 1.25);
    const shiftedInTime = cellPhase(params, { col: 3, row: 4 }, 1.25 + params.wave.kx / params.waveFrequency);
    expect(rightNeighbour.scale).toBeCloseTo(shiftedInTime.scale, 12);
    expect(rightNeighbour.alpha).toBeCloseTo(shiftedInTime.alpha, 12);

    const belowNeighbour = cellPhase(params, { col: 3, row: 5 }, 1.25);
    const shiftedByRow = cellPhase(params, { col: 3, row: 4 }, 1.25 + params.wave.ky / params.waveFrequency);
    expect(belowNeighbour.scale).toBeCloseTo(shiftedByRow.scale, 12);

    expect(rightNeighbour.scale).not.toBeCloseTo(here.scale, 6);
  });

  it("keeps alpha paintable and scale positive across a whole lattice and a whole loop", () => {
    for (const seed of [1, 2, 3, 101, 6007]) {
      const drawn = motionParams(seed);
      for (let step = 0; step < 60; step++) {
        for (let col = -3; col <= 3; col++) {
          for (let row = -3; row <= 3; row++) {
            const { scale, alpha } = cellPhase(drawn, { col, row }, step * 0.25);
            expect(scale).toBeGreaterThan(0);
            expect(alpha).toBeGreaterThanOrEqual(0);
            expect(alpha).toBeLessThanOrEqual(1);
          }
        }
      }
    }
  });

  it("touches neither the params nor the cell it was handed", () => {
    const cell = { col: 2, row: 2 };
    const before = JSON.stringify({ params, cell });
    cellPhase(params, cell, 3);
    expect(JSON.stringify({ params, cell })).toBe(before);
  });
});
