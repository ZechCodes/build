// Pose maths for the device stage. No three.js here: the choreography, the
// tests and the stage all agree on these numbers, and nothing needs a renderer
// to work them out.
import { DEVICE_CONTRACT } from "./device-contract.js";

export const POSE_DEFAULTS = Object.freeze({
  x: 50,
  y: 58,
  w: 0,
  yaw: 0,
  pitch: 0,
  roll: 0,
  opacity: 0,
  lidOpen: 1,
  faceCamera: 1,
});

// One vertical world unit spans the stage at z = 0, so a pose's y percent is a
// percent of stage height and its w percent is a percent of stage width.
export const STAGE_CAMERA = Object.freeze({
  fovDeg: 20,
  near: 0.5,
  far: 8,
  position: Object.freeze({ x: 0, y: 0, z: 0.5 / Math.tan(Math.PI / 18) }),
});

const DEGREES = Math.PI / 180;

function clamp(value, minimum = 0, maximum = 1) {
  return Math.min(maximum, Math.max(minimum, value));
}

function number(value, fallback) {
  return Number.isFinite(value) ? value : fallback;
}

function mix(from, to, amount) {
  return from + (to - from) * amount;
}

export function smoothstep(value) {
  const bounded = clamp(value);
  return bounded * bounded * (3 - 2 * bounded);
}

export function normalizePose(pose = {}) {
  return {
    x: number(pose.x, POSE_DEFAULTS.x),
    y: number(pose.y, POSE_DEFAULTS.y),
    w: number(pose.w, POSE_DEFAULTS.w),
    yaw: number(pose.yaw, POSE_DEFAULTS.yaw),
    pitch: number(pose.pitch, POSE_DEFAULTS.pitch),
    roll: number(pose.roll, POSE_DEFAULTS.roll),
    opacity: clamp(number(pose.opacity, POSE_DEFAULTS.opacity)),
    lidOpen: clamp(number(pose.lidOpen, POSE_DEFAULTS.lidOpen)),
    faceCamera: clamp(number(pose.faceCamera, POSE_DEFAULTS.faceCamera)),
  };
}

// Linear on purpose: GSAP owns the easing, so two timelines that share a pose
// stay in step. Use smoothstep() on the amount for the old stage's feel.
export function interpolatePose(from, to, amount) {
  const start = normalizePose(from);
  const end = normalizePose(to);
  const eased = clamp(amount);
  return Object.fromEntries(
    Object.keys(POSE_DEFAULTS).map((key) => [key, mix(start[key], end[key], eased)]),
  );
}

const LAPTOP = DEVICE_CONTRACT.devices.laptop;

// The exported hinge runs from 90 degrees (closed) to -15 degrees (open): 105
// degrees of travel, measured as an angle off the keyboard deck by
// laptopLidAngleDegrees.
export function laptopLidRotationDegrees(lidOpen = 1) {
  return mix(LAPTOP.lid_hinge_closed_rotation_deg, LAPTOP.lid_hinge_default_rotation_deg, clamp(lidOpen));
}

export function laptopLidAngleDegrees(lidOpen = 1) {
  return LAPTOP.lid_hinge_closed_rotation_deg - laptopLidRotationDegrees(lidOpen);
}

function device(deviceName) {
  const found = DEVICE_CONTRACT.devices[deviceName];
  if (!found) throw new Error(`unknown device ${deviceName}`);
  return found;
}

export function deviceBoundsWidth(deviceName) {
  return device(deviceName).bounds_size_m[0];
}

export function deviceScreenAspect(deviceName) {
  const [width, height] = device(deviceName).screen.size_m;
  return width / height;
}

// An overlay is authored as a percent of stage width covering the display; the
// pose has to carry the enclosure around it.
export function poseWidthForScreenWidth(deviceName, screenWidthPercent) {
  const found = device(deviceName);
  return screenWidthPercent * found.bounds_size_m[0] / found.screen.size_m[0];
}

export function screenWidthForPoseWidth(deviceName, poseWidthPercent) {
  const found = device(deviceName);
  return poseWidthPercent * found.screen.size_m[0] / found.bounds_size_m[0];
}

// Where the model sits relative to its pose position. The laptop hangs from the
// centre of its overall bounds; a slab device puts its glass on the layout
// plane so perspective preserves an HTML alignment exactly.
export function deviceAnchor(deviceName) {
  const found = device(deviceName);
  if (deviceName === "laptop") {
    return [0, found.bounds_size_m[1] / 2, found.body_size_m[2] / 2];
  }
  return [0, 0, found.screen.center_m[2] || 0];
}

function cameraFacingRotation(pose, position) {
  const camera = STAGE_CAMERA.position;
  const depth = camera.z - position.z;
  const amount = pose.faceCamera;
  return {
    pitch: -Math.atan2(camera.y - position.y, depth) * amount + pose.pitch * DEGREES,
    yaw: Math.atan2(-position.x, depth) * amount + pose.yaw * DEGREES,
    roll: pose.roll * DEGREES,
    order: "YXZ",
  };
}

export function poseToWorld(deviceName, candidate, { aspect }) {
  const pose = normalizePose(candidate);
  const position = {
    x: (pose.x / 100 - 0.5) * aspect,
    y: 0.5 - pose.y / 100,
    z: 0,
  };
  return {
    position,
    scale: (pose.w / 100) * aspect / deviceBoundsWidth(deviceName),
    rotation: cameraFacingRotation(pose, position),
    opacity: pose.opacity,
    lidRotation: laptopLidRotationDegrees(pose.lidOpen) * DEGREES,
  };
}

function rotateAboutHingeX([x, y, z], pivot, radians) {
  const cos = Math.cos(radians);
  const sin = Math.sin(radians);
  const dy = y - pivot[1];
  const dz = z - pivot[2];
  return [x, pivot[1] + dy * cos - dz * sin, pivot[2] + dy * sin + dz * cos];
}

// three.js composes a 'YXZ' euler as Ry * Rx * Rz.
function applyEulerYXZ([x, y, z], { yaw, pitch, roll }) {
  const [cz, sz] = [Math.cos(roll), Math.sin(roll)];
  const [cx, sx] = [Math.cos(pitch), Math.sin(pitch)];
  const [cy, sy] = [Math.cos(yaw), Math.sin(yaw)];
  const rolled = [x * cz - y * sz, x * sz + y * cz, z];
  const pitched = [rolled[0], rolled[1] * cx - rolled[2] * sx, rolled[1] * sx + rolled[2] * cx];
  return [
    pitched[0] * cy + pitched[2] * sy,
    pitched[1],
    -pitched[0] * sy + pitched[2] * cy,
  ];
}

// The display rectangle in model space. The exported laptop corners are baked
// at the open hinge angle, so any other lidOpen swings them about the hinge.
export function screenCornersModel(deviceName, lidOpen = 1) {
  const found = device(deviceName);
  const corners = found.screen.corners_m.map((corner) => corner.slice(0, 3));
  if (!found.lid_hinge_node) return corners;
  const delta = (laptopLidRotationDegrees(lidOpen) - found.lid_hinge_default_rotation_deg) * DEGREES;
  return corners.map((corner) => rotateAboutHingeX(corner, found.hinge_pivot_m, delta));
}

export function projectToPixels(point, viewport) {
  const aspect = viewport.width / viewport.height;
  const focal = 1 / Math.tan(STAGE_CAMERA.fovDeg * DEGREES / 2);
  const depth = STAGE_CAMERA.position.z - point[2];
  const ndcX = point[0] * focal / aspect / depth;
  const ndcY = (point[1] - STAGE_CAMERA.position.y) * focal / depth;
  return [(ndcX + 1) / 2 * viewport.width, (1 - ndcY) / 2 * viewport.height];
}

// The four display corners in CSS pixels, in the exported order: bottom left,
// bottom right, top right, top left as the device sees them. Align an HTML
// overlay to these and it sits on the glass.
export function screenCornersPx(deviceName, candidate, viewport) {
  const pose = normalizePose(candidate);
  const world = poseToWorld(deviceName, pose, { aspect: viewport.width / viewport.height });
  const anchor = deviceAnchor(deviceName);
  return screenCornersModel(deviceName, pose.lidOpen).map((corner) => {
    const local = corner.map((value, index) => (value - anchor[index]) * world.scale);
    const rotated = applyEulerYXZ(local, world.rotation);
    return projectToPixels(
      [rotated[0] + world.position.x, rotated[1] + world.position.y, rotated[2] + world.position.z],
      viewport,
    );
  });
}
