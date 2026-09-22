import assert from "node:assert/strict";
import test from "node:test";
import {
  frameCostFromSamples,
  getContainedTextureLayout,
  modelUrl,
  renderSolidDeviceFades,
  screenUrl,
} from "../../src/stage/render.js";

test("model and screen urls hang off the configured bases", () => {
  assert.equal(modelUrl("/landing/assets/devices", "laptop"), "/landing/assets/devices/laptop-low.glb");
  assert.equal(modelUrl("/landing/assets/devices/", "tablet"), "/landing/assets/devices/tablet.glb");
  assert.equal(modelUrl("/landing/assets/devices", "phone"), "/landing/assets/devices/phone.glb");
  assert.throws(() => modelUrl("/x", "watch"), /watch/);
  assert.equal(screenUrl("/landing/assets/screens", "ui10-editor-macbook"), "/landing/assets/screens/ui10-editor-macbook.webp");
  assert.equal(screenUrl("/landing/assets/screens/", "ui02-iphone.webp"), "/landing/assets/screens/ui02-iphone.webp");
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

test("a frame cost is the median sample, so one stall does not condemn a machine", () => {
  assert.equal(frameCostFromSamples([40, 9, 10, 11, 60]), 11);
  assert.equal(frameCostFromSamples([8, 10, 12, 14]), 11);
  assert.equal(frameCostFromSamples([7]), 7);
  assert.equal(frameCostFromSamples([]), 0);
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

  renderSolidDeviceFades(renderer, scene, camera, depthMaterial, [["laptop", laptop]]);

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

  renderSolidDeviceFades(renderer, scene, camera, depthMaterial, [["laptop", laptop], ["phone", phone]]);

  assert.deepEqual(events, [
    { type: "render", overrideMaterial: null, autoClear: true, laptopVisible: true, phoneVisible: false },
    { type: "clear-depth", autoClear: false },
    { type: "render", overrideMaterial: depthMaterial, autoClear: false, laptopVisible: false, phoneVisible: true },
    { type: "render", overrideMaterial: null, autoClear: false, laptopVisible: false, phoneVisible: true },
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

  renderSolidDeviceFades(renderer, scene, {}, { name: "unused at full opacity" }, [["laptop", laptop], ["phone", phone]]);

  assert.deepEqual(events, ["laptop", "clear-depth", "phone"]);
});

test("one fully opaque device keeps the single-pass render path", () => {
  const scene = { overrideMaterial: null };
  const renders = [];
  const renderer = {
    autoClear: true,
    shadowMap: { autoUpdate: true },
    render(currentScene) {
      renders.push(currentScene.overrideMaterial);
    },
  };

  renderSolidDeviceFades(renderer, scene, {}, { name: "device fade depth" }, [
    ["laptop", { opacity: 1, root: { visible: true } }],
  ]);

  assert.deepEqual(renders, [null]);
});
