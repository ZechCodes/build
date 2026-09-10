// Interlocking 2D tilings for the agent bubble: the field of cells a renderer
// paints inside a ~28-40px circle to give an agent its face.
//
// Every generator here is pure — (cellSize, bounds) in, a fresh array of cells
// out — and works in UNTRANSFORMED lattice coordinates. The renderer owns the
// transform: it translates and rotates the whole field under the circle, so the
// field has to reach past the bounds it was asked for. Each generator overscans
// by two whole cells on every side; rotate the result about the bounds' center
// and the corners are still covered.
//
// A cell is:
//   { polygon: [[x, y], ...], center: [x, y], row, col, kind }
//
// `polygon` walks the outline in one direction with no repeated closing vertex.
// `row`/`col` are the lattice indices, which is what an animation reads to give
// a cell its phase (a diagonal wave is `row + col`). `kind` names the cell's
// shape; only the truncated-square tiling mixes two of them, and there the
// filler squares are meant to be drawn more faintly than the octagons.
//
// Interlocking is the whole point: adjacent cells share COMPLETE edges, meaning
// two vertices that agree to within 1e-6. The vertex math below is written to
// land on the same lattice multiples from either side rather than accumulating
// through per-cell offsets, so shared vertices agree to within an ulp or two.

/** How far past the bounds every tiling reaches, counted in cells. Two rather
 *  than one: the renderer rotates the field, and a rotated square's corner
 *  swings out by more than half a cell. */
const OVERSCAN_CELLS = 2;

const SQRT_3 = Math.sqrt(3);

/** The rect a tiling has to cover, with an origin that defaults to (0, 0). */
function rectOf(bounds) {
  const left = Number(bounds && bounds.x) || 0;
  const top = Number(bounds && bounds.y) || 0;
  const width = Math.max(0, Number(bounds && bounds.width) || 0);
  const height = Math.max(0, Number(bounds && bounds.height) || 0);
  return { left, top, right: left + width, bottom: top + height };
}

/** The inclusive index range of a lattice of the given step that spans
 *  [min, max] plus the overscan. `perCell` says how many steps make up one
 *  cell, for lattices (diamonds, triangles) that index half-cells. */
function latticeRange(min, max, step, perCell = 1) {
  const pad = OVERSCAN_CELLS * perCell;
  return [Math.floor(min / step) - pad, Math.ceil(max / step) + pad];
}

/** The mean of a polygon's vertices — for every shape here that is also its
 *  center of symmetry, which is what the renderer scales and spins a cell
 *  about. */
function centroid(polygon) {
  let x = 0;
  let y = 0;
  for (const vertex of polygon) {
    x += vertex[0];
    y += vertex[1];
  }
  return [x / polygon.length, y / polygon.length];
}

/** Whether a point lies inside an outline, by ray casting. Convex or concave,
 *  either winding. A point exactly on an edge is undefined — callers that care
 *  (hit-testing, coverage probes) should keep off the lattice. */
export function pointInPolygon(point, polygon) {
  const [x, y] = point;
  let inside = false;
  for (let i = 0, j = polygon.length - 1; i < polygon.length; j = i, i += 1) {
    const [xi, yi] = polygon[i];
    const [xj, yj] = polygon[j];
    const straddles = yi > y !== yj > y;
    if (straddles && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

/**
 * The axis-aligned square lattice. `cellSize` is the edge.
 *
 * The plainest of the five, and the one a viewer reads as "grid" before they
 * read it as anything else.
 */
export function squares(cellSize, bounds) {
  const rect = rectOf(bounds);
  const [colMin, colMax] = latticeRange(rect.left, rect.right, cellSize);
  const [rowMin, rowMax] = latticeRange(rect.top, rect.bottom, cellSize);
  const cells = [];
  for (let row = rowMin; row <= rowMax; row += 1) {
    for (let col = colMin; col <= colMax; col += 1) {
      const left = col * cellSize;
      const top = row * cellSize;
      const right = (col + 1) * cellSize;
      const bottom = (row + 1) * cellSize;
      cells.push({
        polygon: [[left, top], [right, top], [right, bottom], [left, bottom]],
        center: [(left + right) / 2, (top + bottom) / 2],
        row,
        col,
        kind: "square",
      });
    }
  }
  return cells;
}

/**
 * The square lattice turned 45 degrees: rhombi standing on their points,
 * meeting edge-to-edge. `cellSize` is still the edge.
 *
 * Turning the lattice turns its indices too, so the cells are addressed here by
 * the diagonal pair (a, b) — a counts across, b counts down, and a cell exists
 * where a + b is odd, which is what makes the checkerboard. The square lattice's
 * own row/col fall back out of that pair, so a renderer keying an animation off
 * row/col sees the same numbering it would on `squares`.
 */
export function diamonds(cellSize, bounds) {
  const half = cellSize / Math.SQRT2; // half a diagonal: the edge is the hypotenuse
  const rect = rectOf(bounds);
  const [aMin, aMax] = latticeRange(rect.left, rect.right, half, 2);
  const [bMin, bMax] = latticeRange(rect.top, rect.bottom, half, 2);
  const cells = [];
  for (let b = bMin; b <= bMax; b += 1) {
    for (let a = aMin; a <= aMax; a += 1) {
      if ((((a + b) % 2) + 2) % 2 !== 1) continue;
      cells.push({
        polygon: [
          [a * half, (b - 1) * half],
          [(a + 1) * half, b * half],
          [a * half, (b + 1) * half],
          [(a - 1) * half, b * half],
        ],
        center: [a * half, b * half],
        row: (b - a - 1) / 2,
        col: (a + b - 1) / 2,
        kind: "diamond",
      });
    }
  }
  return cells;
}

// A pointy-top hexagon's six vertices, as multiples of (halfWidth, R). Listed
// from the top point, clockwise.
const HEXAGON_CORNERS = [[0, -1], [1, -0.5], [1, 0.5], [0, 1], [-1, 0.5], [-1, -0.5]];

/**
 * The pointy-top honeycomb. `cellSize` is the edge, which for a regular hexagon
 * is also the distance from center to point.
 *
 * Rows sit 1.5 edges apart and odd rows shift half a cell right — that shift is
 * what makes the rows interlock instead of stacking.
 */
export function hexagons(cellSize, bounds) {
  const halfWidth = (cellSize * SQRT_3) / 2;
  const rowStep = cellSize * 1.5;
  const rect = rectOf(bounds);
  const [colMin, colMax] = latticeRange(rect.left, rect.right, halfWidth * 2);
  const [rowMin, rowMax] = latticeRange(rect.top, rect.bottom, rowStep);
  const cells = [];
  for (let row = rowMin; row <= rowMax; row += 1) {
    const rowIsOdd = (((row % 2) + 2) % 2) === 1;
    for (let col = colMin; col <= colMax; col += 1) {
      const centerX = col * halfWidth * 2 + (rowIsOdd ? halfWidth : 0);
      const centerY = row * rowStep;
      cells.push({
        polygon: HEXAGON_CORNERS.map(([dx, dy]) => [
          centerX + dx * halfWidth,
          centerY + dy * cellSize,
        ]),
        center: [centerX, centerY],
        row,
        col,
        kind: "hexagon",
      });
    }
  }
  return cells;
}

/**
 * Equilateral triangles, alternating point-up and point-down. `cellSize` is the
 * edge; a row is one triangle height tall.
 *
 * `col` counts half-edges across, so consecutive cols are the triangles that
 * actually touch. A cell points up when col + row is even, which flips the
 * alternation every row — that is what lets a row's up-triangles put their
 * bases exactly where the next row's down-triangles put theirs.
 */
export function triangles(cellSize, bounds) {
  const rowHeight = (cellSize * SQRT_3) / 2;
  const halfEdge = cellSize / 2;
  const rect = rectOf(bounds);
  const [colMin, colMax] = latticeRange(rect.left, rect.right, halfEdge, 2);
  const [rowMin, rowMax] = latticeRange(rect.top, rect.bottom, rowHeight);
  const cells = [];
  for (let row = rowMin; row <= rowMax; row += 1) {
    const top = row * rowHeight;
    const bottom = (row + 1) * rowHeight;
    for (let col = colMin; col <= colMax; col += 1) {
      const left = col * halfEdge;
      const right = (col + 2) * halfEdge;
      const middle = (col + 1) * halfEdge;
      const pointsUp = (((col + row) % 2) + 2) % 2 === 0;
      const polygon = pointsUp
        ? [[left, bottom], [right, bottom], [middle, top]]
        : [[left, top], [right, top], [middle, bottom]];
      cells.push({ polygon, center: centroid(polygon), row, col, kind: "triangle" });
    }
  }
  return cells;
}

/**
 * The truncated square tiling (4.8.8): regular octagons with a small square
 * turned 45 degrees filling each gap between four of them. `cellSize` is the
 * edge, shared by both shapes.
 *
 * Both shapes come back in one array, each carrying `kind` ('octagon' or
 * 'square'), so the renderer can hold the fillers back — they read as grout
 * rather than as cells. Cells are emitted octagon-then-its-own-filler in
 * row-major order, so a wave keyed off row/col still sweeps cleanly.
 *
 * The filler at (row, col) sits down-and-right of the octagon at (row, col),
 * which is why the two share those indices.
 */
export function octagons(cellSize, bounds) {
  const period = cellSize * (1 + Math.SQRT2); // center to center, octagon to octagon
  const halfEdge = cellSize / 2;
  const halfPeriod = period / 2;
  const fillerRadius = halfPeriod - halfEdge; // half the filler's diagonal
  const rect = rectOf(bounds);
  const [colMin, colMax] = latticeRange(rect.left, rect.right, period);
  const [rowMin, rowMax] = latticeRange(rect.top, rect.bottom, period);
  const cells = [];
  for (let row = rowMin; row <= rowMax; row += 1) {
    for (let col = colMin; col <= colMax; col += 1) {
      const centerX = col * period;
      const centerY = row * period;
      cells.push({
        // Clockwise from the right end of the top edge. Every vertex is half an
        // edge off one axis and half a period off the other.
        polygon: [
          [centerX + halfEdge, centerY - halfPeriod],
          [centerX + halfPeriod, centerY - halfEdge],
          [centerX + halfPeriod, centerY + halfEdge],
          [centerX + halfEdge, centerY + halfPeriod],
          [centerX - halfEdge, centerY + halfPeriod],
          [centerX - halfPeriod, centerY + halfEdge],
          [centerX - halfPeriod, centerY - halfEdge],
          [centerX - halfEdge, centerY - halfPeriod],
        ],
        center: [centerX, centerY],
        row,
        col,
        kind: "octagon",
      });
      const fillerX = centerX + halfPeriod;
      const fillerY = centerY + halfPeriod;
      cells.push({
        polygon: [
          [fillerX, fillerY - fillerRadius],
          [fillerX + fillerRadius, fillerY],
          [fillerX, fillerY + fillerRadius],
          [fillerX - fillerRadius, fillerY],
        ],
        center: [fillerX, fillerY],
        row,
        col,
        kind: "square",
      });
    }
  }
  return cells;
}

/** The five tilings by name, for a caller that wants to pick one outright. */
export const TILINGS = { squares, diamonds, hexagons, triangles, octagons };

// The pattern ordinals from core/agentRailModel.js `agentPattern` (1..5), in the
// order the agents on a rail take them. The rail hands them out in sequence, so
// what matters is that CONSECUTIVE ordinals look nothing alike: the grid, then
// the honeycomb, then the sharp alternating triangles, then the rhombi, then the
// busy 4.8.8. Squares and diamonds are the one confusable pair — the same shape,
// one of them turned — so they sit at 1 and 4, three agents apart.
const TILING_BY_PATTERN = [squares, hexagons, triangles, diamonds, octagons];

/** The tiling an agent's pattern ordinal draws. Anything outside 1..5 folds
 *  back onto the five the same way `agentPattern` does, so a caller never has
 *  to guard the number it read off a bubble. */
export function tilingForPattern(patternIndex) {
  const place = Math.max(1, Number(patternIndex) || 1);
  return TILING_BY_PATTERN[(place - 1) % TILING_BY_PATTERN.length];
}
