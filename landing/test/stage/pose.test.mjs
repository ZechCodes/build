import assert from "node:assert/strict";
import test from "node:test";
import {
  POSE_DEFAULTS,
  STAGE_CAMERA,
  interpolatePose,
  laptopLidAngleDegrees,
  laptopLidRotationDegrees,
  normalizePose,
  poseToWorld,
  poseWidthForScreenWidth,
  screenCornersPx,
  screenWidthForPoseWidth,
} from "../../src/stage/pose.js";
import { DEVICE_CONTRACT } from "../../src/stage/device-contract.js";

const close = (actual, expected, tolerance, what = "") => {
  assert(
    Math.abs(actual - expected) <= tolerance,
    `${what}: expected ${expected} +/- ${tolerance}, got ${actual}`,
  );
};

test("the hinge maps lidOpen across the exported 105 degrees of travel", () => {
  assert.equal(laptopLidRotationDegrees(0), 90);
  assert.equal(laptopLidRotationDegrees(0.5), 37.5);
  assert.equal(laptopLidRotationDegrees(1), -15);
  assert.equal(laptopLidRotationDegrees(0.7), 16.5);
  // The hero entrance is authored as an angle off the base: 0.7 is about 75 degrees.
  assert.equal(laptopLidAngleDegrees(0.7), 73.5);
  assert.equal(laptopLidAngleDegrees(1), 105);
  assert.equal(laptopLidRotationDegrees(2), -15, "lidOpen is clamped");
  assert.equal(laptopLidRotationDegrees(-1), 90, "lidOpen is clamped");
});

test("a pose fills its defaults and clamps its unit ranges", () => {
  const pose = normalizePose({ x: 65, y: 57, w: 52, opacity: 4, lidOpen: -3, faceCamera: 0.25 });
  assert.equal(pose.x, 65);
  assert.equal(pose.yaw, 0);
  assert.equal(pose.pitch, 0);
  assert.equal(pose.roll, 0);
  assert.equal(pose.opacity, 1);
  assert.equal(pose.lidOpen, 0);
  assert.equal(pose.faceCamera, 0.25);
  assert.equal(normalizePose().w, POSE_DEFAULTS.w);
  assert.equal(normalizePose({ x: "nonsense" }).x, POSE_DEFAULTS.x);
});

test("poses interpolate linearly so a timeline owns the easing", () => {
  const from = { x: 20, y: 60, w: 30, yaw: 0, pitch: 0, roll: 0, opacity: 0, lidOpen: 0.7 };
  const to = { x: 60, y: 50, w: 50, yaw: 10, pitch: -4, roll: 2, opacity: 1, lidOpen: 1 };
  const middle = interpolatePose(from, to, 0.5);
  assert.deepEqual(middle, {
    x: 40, y: 55, w: 40, yaw: 5, pitch: -2, roll: 1, opacity: 0.5, lidOpen: 0.85, faceCamera: 1,
  });
  assert.deepEqual(interpolatePose(from, to, 0), normalizePose(from));
  assert.deepEqual(interpolatePose(from, to, 1), normalizePose(to));
  assert.deepEqual(interpolatePose(from, to, 3), normalizePose(to), "amount is clamped");
});

test("a stage-percent pose becomes a world placement for the 20 degree camera", () => {
  const world = poseToWorld("laptop", { x: 65, y: 57, w: 52 }, { aspect: 1440 / 900 });
  close(STAGE_CAMERA.position.z, 2.8356409098, 1e-9, "camera distance");
  close(world.position.x, 0.24, 1e-12, "world x");
  close(world.position.y, -0.07, 1e-12, "world y");
  assert.equal(world.position.z, 0);
  close(world.scale, 2.6594217142876744, 1e-12, "scale");
  // faceCamera defaults to full compensation: the device turns toward the lens.
  close(world.rotation.yaw, -0.08443571845073192, 1e-12, "yaw");
  close(world.rotation.pitch, -0.024680764729614845, 1e-12, "pitch");
  assert.equal(world.rotation.roll, 0);
  assert.equal(world.rotation.order, "YXZ");
  close(world.lidRotation, Math.PI * -15 / 180, 1e-12, "lid rotation");
});

test("faceCamera 0 keeps the authored angles exactly", () => {
  const pose = { x: 20, y: 70, w: 40, yaw: 12, pitch: -4, roll: 3, faceCamera: 0 };
  const world = poseToWorld("tablet", pose, { aspect: 1.6 });
  close(world.rotation.yaw, 12 * Math.PI / 180, 1e-12, "yaw");
  close(world.rotation.pitch, -4 * Math.PI / 180, 1e-12, "pitch");
  close(world.rotation.roll, 3 * Math.PI / 180, 1e-12, "roll");
});

test("screen width percent converts to pose width through the physical bezel", () => {
  const tablet = DEVICE_CONTRACT.devices.tablet;
  const poseWidth = poseWidthForScreenWidth("tablet", 55);
  close(poseWidth * tablet.screen.size_m[0] / tablet.bounds_size_m[0], 55, 1e-12, "screen width");
  close(screenWidthForPoseWidth("tablet", poseWidth), 55, 1e-12, "round trip");
  assert(poseWidth > 55, "the enclosure is wider than its display");
});

test("the tablet display corners land on the rectangle an HTML overlay uses", () => {
  const viewport = { width: 1440, height: 900 };
  const screenWidthPercent = 55;
  const pose = {
    x: 50,
    y: 60,
    w: poseWidthForScreenWidth("tablet", screenWidthPercent),
    faceCamera: 0,
    opacity: 1,
  };
  const corners = screenCornersPx("tablet", pose, viewport);
  const tablet = DEVICE_CONTRACT.devices.tablet;
  const centerX = viewport.width / 2;
  const centerY = 0.6 * viewport.height;
  const width = screenWidthPercent / 100 * viewport.width;
  const height = width / (tablet.screen.size_m[0] / tablet.screen.size_m[1]);
  const expected = [
    [centerX - width / 2, centerY + height / 2],
    [centerX + width / 2, centerY + height / 2],
    [centerX + width / 2, centerY - height / 2],
    [centerX - width / 2, centerY - height / 2],
  ];
  corners.forEach(([x, y], index) => {
    close(x, expected[index][0], 1e-6, `corner ${index} x`);
    close(y, expected[index][1], 1e-6, `corner ${index} y`);
  });
});

test("the open laptop display leans fifteen degrees back from vertical", () => {
  const viewport = { width: 1440, height: 900 };
  // Put the display centre at the camera height so only the lean foreshortens it.
  const pose = { x: 50, y: 50, w: 52, faceCamera: 0, lidOpen: 1, opacity: 1 };
  const [bottomLeft, bottomRight, topRight, topLeft] = screenCornersPx("laptop", pose, viewport);
  const laptop = DEVICE_CONTRACT.devices.laptop;
  const displayAspect = laptop.screen.size_m[0] / laptop.screen.size_m[1];
  const bottomWidth = bottomRight[0] - bottomLeft[0];
  const topWidth = topRight[0] - topLeft[0];
  const height = (bottomLeft[1] + bottomRight[1]) / 2 - (topLeft[1] + topRight[1]) / 2;
  assert(height > 0, "the display is upright on screen");
  assert(topWidth < bottomWidth, "the top edge leans away from the lens");
  const leanRatio = height / ((bottomWidth + topWidth) / 2) * displayAspect;
  close(leanRatio, Math.cos(15 * Math.PI / 180), 0.02, "projected lean");
});

test("closing the lid folds the display flat onto the keyboard", () => {
  const viewport = { width: 1440, height: 900 };
  const pose = { x: 50, y: 50, w: 52, faceCamera: 0, lidOpen: 0, opacity: 1 };
  const corners = screenCornersPx("laptop", pose, viewport);
  const top = Math.min(...corners.map(([, y]) => y));
  const bottom = Math.max(...corners.map(([, y]) => y));
  const left = Math.min(...corners.map(([x]) => x));
  const right = Math.max(...corners.map(([x]) => x));
  assert(bottom - top < (right - left) * 0.08, "a closed lid is seen edge on");
});
