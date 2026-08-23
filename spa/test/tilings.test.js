import { describe, it, expect } from "vitest";
import {
  squares,
  diamonds,
  hexagons,
  triangles,
  octagons,
  pointInPolygon,
  tilingForPattern,
  TILINGS,
} from "../src/core/tilings.js";
import { agentPattern, AGENT_PATTERN_COUNT } from "../src/core/agentRailModel.js";

const EPSILON = 1e-6;
const BOUNDS = { width: 34, height: 34 };
const CELL = 7;

/** Two lattice points are the same point when they agree to the module's own
 *  tolerance — the test's whole claim about interlocking rests on this. */
function samePoint(a, b) {
  return Math.abs(a[0] - b[0]) <= EPSILON && Math.abs(a[1] - b[1]) <= EPSILON;
}

/** How much of an edge two cells hold in common: 2 is a full shared edge, 1 is
 *  a corner touch, 0 is no contact. */
function sharedVertexCount(cellA, cellB) {
  return cellA.polygon.filter((vertex) => cellB.polygon.some((other) => samePoint(vertex, other)))
    .length;
}

function centerInside(cell, bounds) {
  const [x, y] = cell.center;
  return x > 0 && x < bounds.width && y > 0 && y < bounds.height;
}

/** Probe points spread over the bounds, offset off the lattice so a probe never
 *  lands on a shared edge where "inside" is a coin flip. */
function probePoints(bounds, steps = 11) {
  const points = [];
  for (let row = 0; row < steps; row += 1) {
    for (let col = 0; col < steps; col += 1) {
      points.push([((col + 0.5) * bounds.width) / steps, ((row + 0.5) * bounds.height) / steps]);
    }
  }
  return points;
}

const TILING_CASES = [
  { name: "squares", generate: squares, vertexCount: () => 4, neighbourCount: () => 4 },
  { name: "diamonds", generate: diamonds, vertexCount: () => 4, neighbourCount: () => 4 },
  { name: "hexagons", generate: hexagons, vertexCount: () => 6, neighbourCount: () => 6 },
  { name: "triangles", generate: triangles, vertexCount: () => 3, neighbourCount: () => 3 },
  {
    name: "octagons",
    generate: octagons,
    vertexCount: (cell) => (cell.kind === "octagon" ? 8 : 4),
    neighbourCount: (cell) => (cell.kind === "octagon" ? 8 : 4),
  },
];

describe.each(TILING_CASES)("$name", ({ name, generate, vertexCount, neighbourCount }) => {
  const cells = generate(CELL, BOUNDS);

  it("returns cells with a polygon, a center, and lattice indices", () => {
    expect(cells.length).toBeGreaterThan(0);
    for (const cell of cells) {
      expect(Number.isInteger(cell.row)).toBe(true);
      expect(Number.isInteger(cell.col)).toBe(true);
      expect(cell.center).toHaveLength(2);
      expect(cell.center.every(Number.isFinite)).toBe(true);
      expect(cell.polygon.every((vertex) => vertex.length === 2 && vertex.every(Number.isFinite)))
        .toBe(true);
    }
  });

  it("gives every polygon the vertex count its shape calls for", () => {
    for (const cell of cells) expect(cell.polygon).toHaveLength(vertexCount(cell));
  });

  it("puts each center at the mean of its own vertices", () => {
    for (const cell of cells) {
      const meanX = cell.polygon.reduce((sum, [x]) => sum + x, 0) / cell.polygon.length;
      const meanY = cell.polygon.reduce((sum, [, y]) => sum + y, 0) / cell.polygon.length;
      expect(cell.center[0]).toBeCloseTo(meanX, 6);
      expect(cell.center[1]).toBeCloseTo(meanY, 6);
    }
  });

  it("interlocks: every cell over the bounds shares a full edge with each neighbour", () => {
    const interior = cells.filter((cell) => centerInside(cell, BOUNDS));
    expect(interior.length).toBeGreaterThan(3);
    for (const cell of interior) {
      let fullEdges = 0;
      for (const other of cells) {
        if (other === cell) continue;
        const shared = sharedVertexCount(cell, other);
        // Three shared vertices would mean two cells sitting on top of one another.
        expect(shared).toBeLessThanOrEqual(2);
        if (shared === 2) fullEdges += 1;
      }
      expect(fullEdges).toBe(neighbourCount(cell));
    }
  });

  it("covers the bounds: every probe point falls inside some cell", () => {
    for (const probe of probePoints(BOUNDS)) {
      const hits = cells.filter((cell) => pointInPolygon(probe, cell.polygon));
      expect(hits.length, `${name} left ${probe} uncovered`).toBeGreaterThan(0);
    }
  });

  it("overscans by at least one whole cell on every side", () => {
    expect(cells.some((cell) => cell.polygon.every(([x]) => x < 0))).toBe(true);
    expect(cells.some((cell) => cell.polygon.every(([x]) => x > BOUNDS.width))).toBe(true);
    expect(cells.some((cell) => cell.polygon.every(([, y]) => y < 0))).toBe(true);
    expect(cells.some((cell) => cell.polygon.every(([, y]) => y > BOUNDS.height))).toBe(true);
  });

  it("is pure: same arguments, same field, and the bounds come back untouched", () => {
    const bounds = { width: 34, height: 34 };
    const first = generate(CELL, bounds);
    const second = generate(CELL, bounds);
    expect(first).toEqual(second);
    expect(bounds).toEqual({ width: 34, height: 34 });
  });

  it("honours a bounds origin by covering the offset rect", () => {
    const offset = generate(CELL, { x: 100, y: 60, width: 34, height: 34 });
    for (const probe of probePoints(BOUNDS, 5)) {
      const moved = [probe[0] + 100, probe[1] + 60];
      expect(offset.some((cell) => pointInPolygon(moved, cell.polygon))).toBe(true);
    }
  });
});

describe("squares", () => {
  it("meets its right-hand neighbour along a whole edge", () => {
    const cells = squares(CELL, BOUNDS);
    const left = cells.find((cell) => cell.row === 0 && cell.col === 0);
    const right = cells.find((cell) => cell.row === 0 && cell.col === 1);
    expect(sharedVertexCount(left, right)).toBe(2);
    expect(left.polygon).toEqual([[0, 0], [7, 0], [7, 7], [0, 7]]);
  });
});

describe("diamonds", () => {
  it("is the square lattice turned 45 degrees, so an edge is still cellSize", () => {
    const cells = diamonds(CELL, BOUNDS);
    const cell = cells.find((entry) => centerInside(entry, BOUNDS));
    const [first, second] = cell.polygon;
    expect(Math.hypot(second[0] - first[0], second[1] - first[1])).toBeCloseTo(CELL, 6);
    // A rhombus standing on its point: no edge is axis-aligned.
    for (let i = 0; i < cell.polygon.length; i += 1) {
      const from = cell.polygon[i];
      const to = cell.polygon[(i + 1) % cell.polygon.length];
      expect(Math.abs(to[0] - from[0])).toBeGreaterThan(EPSILON);
      expect(Math.abs(to[1] - from[1])).toBeGreaterThan(EPSILON);
    }
  });
});

describe("hexagons", () => {
  it("draws regular hexagons: every edge is cellSize", () => {
    const cells = hexagons(CELL, BOUNDS);
    const cell = cells.find((entry) => centerInside(entry, BOUNDS));
    for (let i = 0; i < 6; i += 1) {
      const from = cell.polygon[i];
      const to = cell.polygon[(i + 1) % 6];
      expect(Math.hypot(to[0] - from[0], to[1] - from[1])).toBeCloseTo(CELL, 6);
    }
  });
});

describe("triangles", () => {
  it("alternates up and down along a row, and every edge is cellSize", () => {
    const cells = triangles(CELL, BOUNDS);
    const row = cells.filter((cell) => cell.row === 1).sort((a, b) => a.col - b.col);
    // The third vertex is the apex; above the center means the triangle points up.
    const pointsUp = row.map((cell) => cell.polygon[2][1] < cell.center[1]);
    for (let i = 1; i < pointsUp.length; i += 1) expect(pointsUp[i]).not.toBe(pointsUp[i - 1]);
    for (const cell of row) {
      for (let i = 0; i < 3; i += 1) {
        const from = cell.polygon[i];
        const to = cell.polygon[(i + 1) % 3];
        expect(Math.hypot(to[0] - from[0], to[1] - from[1])).toBeCloseTo(CELL, 6);
      }
    }
  });
});

describe("octagons", () => {
  const cells = octagons(CELL, BOUNDS);

  it("returns the octagons and the filler squares, each named by kind", () => {
    const kinds = new Set(cells.map((cell) => cell.kind));
    expect(kinds).toEqual(new Set(["octagon", "square"]));
    expect(cells.filter((cell) => cell.kind === "square").length).toBeGreaterThan(0);
  });

  it("makes both shapes regular with edge cellSize", () => {
    for (const cell of cells.filter((entry) => centerInside(entry, BOUNDS))) {
      for (let i = 0; i < cell.polygon.length; i += 1) {
        const from = cell.polygon[i];
        const to = cell.polygon[(i + 1) % cell.polygon.length];
        expect(Math.hypot(to[0] - from[0], to[1] - from[1])).toBeCloseTo(CELL, 6);
      }
    }
  });

  it("fills the gaps: probes land in octagons and in squares both", () => {
    const hitKinds = new Set();
    for (const probe of probePoints(BOUNDS)) {
      for (const cell of cells) if (pointInPolygon(probe, cell.polygon)) hitKinds.add(cell.kind);
    }
    expect(hitKinds).toEqual(new Set(["octagon", "square"]));
  });
});

describe("pointInPolygon", () => {
  const unitSquare = [[0, 0], [1, 0], [1, 1], [0, 1]];

  it("accepts an interior point and rejects an exterior one", () => {
    expect(pointInPolygon([0.5, 0.5], unitSquare)).toBe(true);
    expect(pointInPolygon([1.5, 0.5], unitSquare)).toBe(false);
    expect(pointInPolygon([0.5, -0.5], unitSquare)).toBe(false);
  });

  it("handles a concave outline without leaking into the notch", () => {
    const chevron = [[0, 0], [4, 0], [4, 4], [2, 1], [0, 4]];
    expect(pointInPolygon([1, 0.5], chevron)).toBe(true);
    expect(pointInPolygon([2, 3], chevron)).toBe(false);
  });
});

describe("tilingForPattern", () => {
  it("gives each of the five agent patterns its own tiling", () => {
    const chosen = [1, 2, 3, 4, 5].map(tilingForPattern);
    expect(new Set(chosen).size).toBe(5);
    expect(new Set(chosen)).toEqual(new Set(Object.values(TILINGS)));
  });

  it("keeps the neighbouring ordinals apart: squares and diamonds are not adjacent", () => {
    const order = [1, 2, 3, 4, 5].map(tilingForPattern);
    expect(Math.abs(order.indexOf(squares) - order.indexOf(diamonds))).toBeGreaterThan(1);
  });

  it("answers for every pattern agentPattern can produce", () => {
    for (let ordinal = 1; ordinal <= AGENT_PATTERN_COUNT * 2 + 1; ordinal += 1) {
      expect(typeof tilingForPattern(agentPattern(ordinal))).toBe("function");
    }
  });

  it("folds an out-of-range or missing pattern back onto the five", () => {
    expect(tilingForPattern(6)).toBe(tilingForPattern(1));
    expect(tilingForPattern(0)).toBe(tilingForPattern(1));
    expect(tilingForPattern(null)).toBe(tilingForPattern(1));
  });
});
