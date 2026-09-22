// Putting HTML on a device screen. The stage projects a display's four corners
// to CSS pixels; this maps a panel authored in texture pixels onto that quad
// with a CSS matrix3d, or part way between the quad and a flat rectangle when
// a close-up lifts out of the screen. Pure maths: no DOM, no three.

// The 3×3 projective transform that carries the unit square's corners
// (0,0) (1,0) (1,1) (0,1) onto the quad [tl, tr, br, bl]. Heckbert's
// square-to-quad, in row-major order.
function squareToQuad([[x0, y0], [x1, y1], [x2, y2], [x3, y3]]) {
  const dx1 = x1 - x2;
  const dx2 = x3 - x2;
  const dx3 = x0 - x1 + x2 - x3;
  const dy1 = y1 - y2;
  const dy2 = y3 - y2;
  const dy3 = y0 - y1 + y2 - y3;
  const det = dx1 * dy2 - dx2 * dy1;
  const g = det === 0 ? 0 : (dx3 * dy2 - dx2 * dy3) / det;
  const h = det === 0 ? 0 : (dx1 * dy3 - dx3 * dy1) / det;
  return [
    [x1 - x0 + g * x1, x3 - x0 + h * x3, x0],
    [y1 - y0 + g * y1, y3 - y0 + h * y3, y0],
    [g, h, 1],
  ];
}

// The transform that carries a width×height rectangle at the origin onto the
// quad [tl, tr, br, bl].
export function homographyFromQuad(width, height, quad) {
  const [[a, b, c], [d, e, f], [g, h, i]] = squareToQuad(quad);
  // Compose with the scale that turns rectangle pixels into unit-square
  // coordinates first.
  return [
    [a / width, b / height, c],
    [d / width, e / height, f],
    [g / width, h / height, i],
  ];
}

export function projectPoint(H, x, y) {
  const w = H[2][0] * x + H[2][1] * y + H[2][2];
  return [
    (H[0][0] * x + H[0][1] * y + H[0][2]) / w,
    (H[1][0] * x + H[1][1] * y + H[1][2]) / w,
  ];
}

// CSS matrix3d is column-major with a z row and column the 2D projective
// transform never touches.
export function matrix3d(H) {
  const [[a, b, c], [d, e, f], [g, h, i]] = H;
  const values = [a, d, 0, g, b, e, 0, h, 0, 0, 1, 0, c, f, 0, i];
  return `matrix3d(${values.map((value) => Number(value.toFixed(6))).join(",")})`;
}

// A region of the texture, [x, y, width, height] in texture pixels, as the
// quad it occupies once the whole screen is projected through H.
export function regionQuad(H, [x, y, width, height]) {
  return [
    projectPoint(H, x, y),
    projectPoint(H, x + width, y),
    projectPoint(H, x + width, y + height),
    projectPoint(H, x, y + height),
  ];
}

// The flat rectangle a lifted close-up lands on: centred on (cx, cy) in CSS
// pixels, `width` pixels wide, keeping the region's aspect.
export function flatQuad([, , width, height], { cx, cy, width: targetWidth }) {
  const targetHeight = targetWidth * height / width;
  const left = cx - targetWidth / 2;
  const top = cy - targetHeight / 2;
  return [
    [left, top],
    [left + targetWidth, top],
    [left + targetWidth, top + targetHeight],
    [left, top + targetHeight],
  ];
}

export function lerpQuad(from, to, amount) {
  const t = Math.min(1, Math.max(0, amount));
  return from.map(([x, y], index) => [
    x + (to[index][0] - x) * t,
    y + (to[index][1] - y) * t,
  ]);
}

// The stage exports display corners as [bottomLeft, bottomRight, topRight,
// topLeft]; a texture's origin is its top left.
export function screenQuadFromCorners(corners) {
  return [corners[3], corners[2], corners[1], corners[0]];
}

// The matrix3d for a panel authored `width`×`height` texture pixels covering
// `region` of a screen whose corners the stage projected, lifted `lift` of the
// way to `flat` (a quad) when given.
export function panelTransform({ screenCorners, textureSize, region, lift = 0, flat = null }) {
  const [textureWidth, textureHeight] = textureSize;
  const screen = homographyFromQuad(textureWidth, textureHeight, screenQuadFromCorners(screenCorners));
  let quad = regionQuad(screen, region);
  if (flat && lift > 0) quad = lerpQuad(quad, flat, lift);
  return matrix3d(homographyFromQuad(region[2], region[3], quad));
}
