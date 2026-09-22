import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  flatQuad,
  homographyFromQuad,
  lerpQuad,
  matrix3d,
  panelTransform,
  projectPoint,
  regionQuad,
  screenQuadFromCorners,
} from "../../src/stage/overlay.js";

const close = (actual, expected, message) => assert.ok(Math.abs(actual - expected) < 1e-6, `${message}: ${actual} vs ${expected}`);
const closeQuad = (actual, expected) => actual.forEach((point, index) => {
  close(point[0], expected[index][0], `x${index}`);
  close(point[1], expected[index][1], `y${index}`);
});

describe("homographyFromQuad", () => {
  it("carries the rectangle's corners onto the quad, including a keystone", () => {
    const quad = [[100, 50], [700, 80], [680, 420], [120, 400]];
    const H = homographyFromQuad(1512, 982, quad);
    closeQuad([
      projectPoint(H, 0, 0),
      projectPoint(H, 1512, 0),
      projectPoint(H, 1512, 982),
      projectPoint(H, 0, 982),
    ], quad);
  });

  it("is the identity scale for an axis-aligned target of the same size", () => {
    const H = homographyFromQuad(200, 100, [[0, 0], [200, 0], [200, 100], [0, 100]]);
    closeQuad([projectPoint(H, 50, 25)], [[50, 25]]);
  });

  it("maps a region's texture corners through the screen's projection", () => {
    const quad = [[100, 50], [700, 80], [680, 420], [120, 400]];
    const H = homographyFromQuad(1512, 982, quad);
    const region = regionQuad(H, [0, 0, 1512, 982]);
    closeQuad(region, quad);
    const inner = regionQuad(H, [756, 491, 10, 10]);
    assert.ok(inner[0][0] > 100 && inner[0][0] < 700);
  });
});

describe("matrix3d", () => {
  it("serialises column-major with the z row and column untouched", () => {
    const value = matrix3d([[1, 2, 3], [4, 5, 6], [7, 8, 9]]);
    assert.equal(value, "matrix3d(1,4,0,7,2,5,0,8,0,0,1,0,3,6,0,9)");
  });
});

describe("lifting", () => {
  it("lerps a quad toward its flat target and clamps the amount", () => {
    const from = [[0, 0], [10, 0], [10, 10], [0, 10]];
    const to = [[100, 100], [200, 100], [200, 200], [100, 200]];
    closeQuad(lerpQuad(from, to, 0.5), [[50, 50], [105, 50], [105, 105], [50, 105]]);
    closeQuad(lerpQuad(from, to, 2), to);
  });

  it("centres the flat rectangle and keeps the region's aspect", () => {
    const quad = flatQuad([0, 0, 400, 200], { cx: 500, cy: 300, width: 800 });
    closeQuad(quad, [[100, 100], [900, 100], [900, 500], [100, 500]]);
  });

  it("reads the stage's corner order as top-left first", () => {
    const corners = [[0, 100], [200, 100], [200, 0], [0, 0]];
    assert.deepEqual(screenQuadFromCorners(corners), [[0, 0], [200, 0], [200, 100], [0, 100]]);
  });

  it("produces a matrix that lands a panel on its region, or on the flat target when lifted", () => {
    const screenCorners = [[100, 400], [700, 420], [680, 60], [120, 50]];
    const region = [200, 100, 400, 300];
    const onScreen = panelTransform({ screenCorners, textureSize: [1512, 982], region });
    assert.match(onScreen, /^matrix3d\(/);
    const flat = flatQuad(region, { cx: 400, cy: 300, width: 800 });
    const lifted = panelTransform({ screenCorners, textureSize: [1512, 982], region, lift: 1, flat });
    const values = lifted.slice(9, -1).split(",").map(Number);
    // Column-major: [a, d, 0, g, b, e, 0, h, 0,0,1,0, c, f, 0, i] — a flat
    // target is a pure scale-and-translate, so the projective terms are zero.
    close(values[3], 0, "g");
    close(values[7], 0, "h");
    close(values[0] / values[15], 2, "scale x");
    close(values[12] / values[15], 0, "left");
    close(values[13] / values[15], 0, "top");
  });
});
