import assert from "node:assert/strict";
import test from "node:test";
import { Euler, MeshBasicMaterial, MeshPhysicalMaterial, PerspectiveCamera, Texture, Vector3 } from "three";
import {
  cloneDeviceSurfaceMaterial,
  createDeviceScreenMaterial,
  getCameraFacingRotation,
  getContainedTextureLayout,
  getDeviceFramePoses,
  getDeviceScreenSource,
  getLaptopLidRotationDegrees,
  renderSolidDeviceFades,
} from "../../skriftapp/buildapp/landing/device-stage.js";
import { DEVICE_CONTRACT } from "../../skriftapp/buildapp/landing/assets/devices/device-contract.js";
import { STORY_SCENES } from "../../skriftapp/buildapp/landing/story-manifest.js";

const hidden = { x: 50, y: 50, w: 0, rotate: [0, 0, 0], opacity: 0 };
const pose = (x, y, w, opacity = 1) => ({ x, y, w, rotate: [0, 0, 0], opacity });
const scene = (laptop, tablet, phone, review = hidden) => ({
  id: "scene",
  poses: { desktop: { laptop, tablet, phone, review } },
});
const scenes = [
  scene(pose(65, 60, 58), hidden, hidden),
  scene(pose(28, 60, 34), hidden, pose(69, 59, 24)),
  scene(hidden, hidden, pose(68, 56, 28)),
  scene(hidden, pose(62, 59, 64), hidden),
  scene(hidden, hidden, hidden, pose(50, 60, 82)),
  scene(pose(30, 68, 34), pose(53, 69, 31), pose(73, 70, 12)),
];

function frame(sceneIndex, local) {
  return { sceneIndex, local, profile: "desktop" };
}

test("the hero opens the laptop from closed and leaves it fully open", () => {
  const closed = getDeviceFramePoses(scenes, frame(0, 0)).laptop;
  const halfway = getDeviceFramePoses(scenes, frame(0, 0.225)).laptop;
  const open = getDeviceFramePoses(scenes, frame(0, 0.45)).laptop;
  assert.equal(closed.w, 58);
  assert.equal(closed.lidOpen, 0);
  assert(Math.abs(halfway.lidOpen - 0.5) < 1e-9);
  assert.equal(open.lidOpen, 1);
  assert.equal(getLaptopLidRotationDegrees(closed.lidOpen), 90);
  assert.equal(getLaptopLidRotationDegrees(halfway.lidOpen), 37.5);
  assert.equal(getLaptopLidRotationDegrees(open.lidOpen), -15);
});

test("adjacent scenes share the same boundary pose", () => {
  for (let sceneIndex = 0; sceneIndex < scenes.length - 1; sceneIndex += 1) {
    const outgoing = getDeviceFramePoses(scenes, frame(sceneIndex, 1));
    const incoming = getDeviceFramePoses(scenes, frame(sceneIndex + 1, 0));
    assert.deepEqual(incoming, outgoing);
  }
});

test("phones and tablets pivot in continuously and settle without idle motion", () => {
  const phoneOutgoing = getDeviceFramePoses(scenes, frame(0, 1)).phone;
  const phoneIncoming = getDeviceFramePoses(scenes, frame(1, 0)).phone;
  assert.deepEqual(phoneIncoming, phoneOutgoing);
  assert.deepEqual(phoneIncoming.rotate, [14, -4, 4]);
  assert.equal(phoneIncoming.opacity, 0.5);
  assert.deepEqual(getDeviceFramePoses(scenes, frame(1, 0.5)).phone.rotate, [0, 0, 0]);
  assert.deepEqual(getDeviceFramePoses(scenes, frame(1, 0.55)).phone.rotate, [0, 0, 0]);

  const tabletOutgoing = getDeviceFramePoses(scenes, frame(2, 1)).tablet;
  const tabletIncoming = getDeviceFramePoses(scenes, frame(3, 0)).tablet;
  assert.deepEqual(tabletIncoming, tabletOutgoing);
  assert.deepEqual(tabletIncoming.rotate, [13, -3, 3]);
  assert.equal(tabletIncoming.opacity, 0.5);
});

test("landscape tablet overview fits the stage and keeps continuous boundaries", () => {
  const stageAspect = 4 / 3;
  const options = { stageAspect };
  const landscapeFrame = (sceneIndex, local) => ({ sceneIndex, local, profile: "tablet" });
  const settled = getDeviceFramePoses(STORY_SCENES, landscapeFrame(3, 0.5), options).tablet;
  const tablet = DEVICE_CONTRACT.devices.tablet;
  const expectedWidth = 30 * tablet.bounds_size_m[0] / tablet.bounds_size_m[1] / stageAspect;
  assert(Math.abs(settled.w - expectedWidth) < 0.000001);
  assert.equal(settled.y, 82);
  assert.deepEqual(
    getDeviceFramePoses(STORY_SCENES, landscapeFrame(2, 1), options),
    getDeviceFramePoses(STORY_SCENES, landscapeFrame(3, 0), options),
  );
  assert.deepEqual(
    getDeviceFramePoses(STORY_SCENES, landscapeFrame(3, 1), options),
    getDeviceFramePoses(STORY_SCENES, landscapeFrame(4, 0), options),
  );

  const portrait = getDeviceFramePoses(
    STORY_SCENES,
    landscapeFrame(3, 0.5),
    { stageAspect: 0.75 },
  ).tablet;
  assert.equal(portrait.w, 62);
  assert.equal(portrait.y, 78);
});

test("review aligns the tablet before fading its hardware", () => {
  const aligned = getDeviceFramePoses(scenes, frame(4, 0)).tablet;
  const tablet = DEVICE_CONTRACT.devices.tablet;
  assert.deepEqual(aligned, {
    ...pose(50, 60, 82 * tablet.bounds_size_m[0] / tablet.screen.size_m[0]),
    faceCamera: 0,
  });
  assert.equal(getDeviceFramePoses(scenes, frame(4, 0.1)).tablet.w, aligned.w);
  assert.equal(getDeviceFramePoses(scenes, frame(4, 0.2)).tablet.opacity, 0);
  assert.equal(getDeviceFramePoses(scenes, frame(4, 0.85)).tablet.opacity, 0);
});

test("all four review display corners project onto the HTML review rectangle", () => {
  const viewport = { width: 1440, height: 900 };
  const aspect = viewport.width / viewport.height;
  const reviewWidth = 55;
  const aligned = getDeviceFramePoses(scenes, frame(4, 0), { reviewWidth }).tablet;
  const tablet = DEVICE_CONTRACT.devices.tablet;
  const camera = new PerspectiveCamera(20, aspect, 0.5, 8);
  camera.position.set(0, 0, 0.5 / Math.tan(Math.PI / 18));
  camera.lookAt(0, 0, 0);
  camera.updateProjectionMatrix();

  const rootPosition = new Vector3(
    ((aligned.x / 100) - 0.5) * aspect,
    0.5 - aligned.y / 100,
    0,
  );
  const rotation = getCameraFacingRotation(aligned, rootPosition, camera.position);
  const rootRotation = new Euler(rotation.pitch, rotation.yaw, rotation.roll, "YXZ");
  const scale = (aligned.w / 100 * aspect) / tablet.bounds_size_m[0];
  const screenCenter = new Vector3(...tablet.screen.center_m);
  const pixels = tablet.screen.corners_m.map((corner) => {
    const point = new Vector3(...corner)
      .sub(screenCenter)
      .multiplyScalar(scale)
      .applyEuler(rootRotation)
      .add(rootPosition)
      .project(camera);
    return [
      (point.x + 1) / 2 * viewport.width,
      (1 - point.y) / 2 * viewport.height,
    ];
  });
  const centerX = aligned.x / 100 * viewport.width;
  const centerY = aligned.y / 100 * viewport.height;
  const screenWidth = reviewWidth / 100 * viewport.width;
  const screenHeight = screenWidth / (tablet.screen.size_m[0] / tablet.screen.size_m[1]);
  const expected = [
    [centerX - screenWidth / 2, centerY + screenHeight / 2],
    [centerX + screenWidth / 2, centerY + screenHeight / 2],
    [centerX + screenWidth / 2, centerY - screenHeight / 2],
    [centerX - screenWidth / 2, centerY - screenHeight / 2],
  ];
  pixels.forEach((point, index) => {
    assert(Math.abs(point[0] - expected[index][0]) < 0.000001);
    assert(Math.abs(point[1] - expected[index][1]) < 0.000001);
  });
});

test("review width uses the physical tablet screen inset", () => {
  const reviewWidth = 55;
  const options = { reviewWidth };
  const outgoing = getDeviceFramePoses(scenes, frame(3, 1), options).tablet;
  const incoming = getDeviceFramePoses(scenes, frame(4, 0), options).tablet;
  assert.deepEqual(incoming, outgoing);
  const tablet = DEVICE_CONTRACT.devices.tablet;
  const projectedScreenWidth = incoming.w * tablet.screen.size_m[0] / tablet.bounds_size_m[0];
  assert(Math.abs(projectedScreenWidth - reviewWidth) < 1e-9);
});

test("screen textures preserve aspect with neutral letterbox space", () => {
  assert.deepEqual(
    getContainedTextureLayout(1600, 1000, 4 / 3),
    { width: 1600, height: 1200, x: 0, y: 100 },
  );
  assert.deepEqual(
    getContainedTextureLayout(1200, 1600, 1),
    { width: 1600, height: 1600, x: 200, y: 0 },
  );
});

test("active screen pixels use an unlit, untone-mapped material", () => {
  const material = createDeviceScreenMaterial({ MeshBasicMaterial });

  assert.equal(material.isMeshBasicMaterial, true);
  assert.equal(material.toneMapped, false);
  assert.equal(material.color.getHex(), 0xffffff);
  assert.equal("emissiveMap" in material, false);
});

test("only front glass gets the restrained reflection contract", () => {
  const sourceMap = new Texture();
  const frontGlass = new MeshPhysicalMaterial({
    map: sourceMap,
    metalness: 0.05,
    roughness: 0.1,
    specularIntensity: 0.6,
    clearcoat: 0.85,
    clearcoatRoughness: 0.08,
  });
  frontGlass.name = "FrontGlass";
  const restrained = cloneDeviceSurfaceMaterial(frontGlass, 16);

  assert.notEqual(restrained, frontGlass);
  assert.equal(restrained.map, sourceMap);
  assert.equal(restrained.map.anisotropy, 8);
  assert.equal(restrained.metalness, 0);
  assert.equal(restrained.roughness, 0.3);
  assert.equal(restrained.specularIntensity, 0.18);
  assert.equal(restrained.clearcoat, 0.1);
  assert.equal(restrained.clearcoatRoughness, 0.3);
  assert.equal(restrained.envMapIntensity, 0.3);
  assert.equal(restrained.transparent, true);
  assert.equal(frontGlass.roughness, 0.1);

  const opticalGlass = new MeshPhysicalMaterial({ roughness: 0.055, clearcoat: 1 });
  opticalGlass.name = "OpticalGlass";
  const untouched = cloneDeviceSurfaceMaterial(opticalGlass, 16);
  assert.equal(untouched.roughness, 0.055);
  assert.equal(untouched.clearcoat, 1);
  assert.equal(untouched.envMapIntensity, 1);
});

test("screen checkpoints resolve canonical maps", () => {
  assert.match(getDeviceScreenSource("phone", { sceneIndex: 0, local: 0.7 }), /ui02-iphone/);
  assert.match(getDeviceScreenSource("phone", { sceneIndex: 2, checkpoint: "question" }), /ui03-question-iphone/);
  assert.match(getDeviceScreenSource("phone", { sceneIndex: 2, checkpoint: "answer" }), /ui03-answer-iphone/);
  assert.match(getDeviceScreenSource("phone", { sceneIndex: 2, checkpoint: "resumed" }), /ui03-resumed-iphone/);
  assert.equal(getDeviceScreenSource("tablet", { sceneIndex: 3 }), "/landing/assets/screens/ui04-ipad.webp");
  assert.equal(getDeviceScreenSource("tablet", { sceneIndex: 2, local: 0.7 }), "/landing/assets/screens/ui04-ipad.webp");
  assert.equal(getDeviceScreenSource("tablet", { sceneIndex: 4, local: 0.5 }), "/landing/assets/screens/ui05-approval-ipad.webp");
  assert.equal(getDeviceScreenSource("tablet", { sceneIndex: 4, local: 0.9 }), "/landing/assets/screens/ui05-merged-ipad.webp");
  assert.match(getDeviceScreenSource("laptop", { sceneIndex: 5 }), /ui05-merged-macbook/);
  assert.equal(getDeviceScreenSource("tablet", { sceneIndex: 5 }), "/landing/assets/screens/ui05-merged-ipad.webp");
});

test("partially faded devices render an occluding depth pass before color", () => {
  const scene = { overrideMaterial: null };
  const camera = {};
  const depthMaterial = { name: "device fade depth" };
  const laptop = { opacity: 0.58, root: { visible: true } };
  const renders = [];
  const renderer = {
    autoClear: true,
    shadowMap: { autoUpdate: true },
    render(currentScene, currentCamera) {
      assert.equal(currentCamera, camera);
      renders.push({
        overrideMaterial: currentScene.overrideMaterial,
        autoClear: this.autoClear,
        shadowAutoUpdate: this.shadowMap.autoUpdate,
      });
    },
  };

  renderSolidDeviceFades(
    renderer,
    scene,
    camera,
    depthMaterial,
    [["laptop", laptop]],
  );

  assert.deepEqual(renders, [
    { overrideMaterial: depthMaterial, autoClear: true, shadowAutoUpdate: true },
    { overrideMaterial: null, autoClear: false, shadowAutoUpdate: false },
  ]);
  assert.equal(scene.overrideMaterial, null);
  assert.equal(renderer.autoClear, true);
  assert.equal(renderer.shadowMap.autoUpdate, true);
  assert.equal(laptop.root.visible, true);
});

test("a near-transparent overlapping device cannot erase an earlier device", () => {
  const scene = { overrideMaterial: null };
  const camera = {};
  const depthMaterial = { name: "device fade depth" };
  const laptop = { opacity: 1, root: { visible: true } };
  const phone = { opacity: 0.016, root: { visible: true } };
  const events = [];
  const renderer = {
    autoClear: true,
    shadowMap: { autoUpdate: true },
    clearDepth() {
      events.push({ type: "clear-depth", autoClear: this.autoClear });
    },
    render(currentScene) {
      events.push({
        type: "render",
        overrideMaterial: currentScene.overrideMaterial,
        autoClear: this.autoClear,
        laptopVisible: laptop.root.visible,
        phoneVisible: phone.root.visible,
      });
    },
  };

  renderSolidDeviceFades(
    renderer,
    scene,
    camera,
    depthMaterial,
    [["laptop", laptop], ["phone", phone]],
  );

  assert.deepEqual(events, [
    {
      type: "render",
      overrideMaterial: null,
      autoClear: true,
      laptopVisible: true,
      phoneVisible: false,
    },
    { type: "clear-depth", autoClear: false },
    {
      type: "render",
      overrideMaterial: depthMaterial,
      autoClear: false,
      laptopVisible: false,
      phoneVisible: true,
    },
    {
      type: "render",
      overrideMaterial: null,
      autoClear: false,
      laptopVisible: false,
      phoneVisible: true,
    },
  ]);
  assert.equal(laptop.root.visible, true);
  assert.equal(phone.root.visible, true);
});

test("overlapping devices keep the same layer order at full opacity", () => {
  const scene = { overrideMaterial: null };
  const laptop = { opacity: 1, root: { visible: true } };
  const phone = { opacity: 1, root: { visible: true } };
  const events = [];
  const renderer = {
    autoClear: true,
    shadowMap: { autoUpdate: true },
    clearDepth() {
      events.push("clear-depth");
    },
    render() {
      events.push(laptop.root.visible ? "laptop" : "phone");
    },
  };

  renderSolidDeviceFades(
    renderer,
    scene,
    {},
    { name: "unused at full opacity" },
    [["laptop", laptop], ["phone", phone]],
  );

  assert.deepEqual(events, ["laptop", "clear-depth", "phone"]);
});

test("one fully opaque device keeps the single-pass render path", () => {
  const scene = { overrideMaterial: null };
  const camera = {};
  const depthMaterial = { name: "device fade depth" };
  const renders = [];
  const renderer = {
    autoClear: true,
    shadowMap: { autoUpdate: true },
    render(currentScene) {
      renders.push(currentScene.overrideMaterial);
    },
  };

  renderSolidDeviceFades(
    renderer,
    scene,
    camera,
    depthMaterial,
    [["laptop", { opacity: 1, root: { visible: true } }]],
  );

  assert.deepEqual(renders, [null]);
});
