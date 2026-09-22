// Render helpers with no three.js in them, so the render order, the letterbox
// and the frame budget can be tested without a GPU.
const MODEL_FILES = Object.freeze({
  laptop: "laptop-low.glb",
  tablet: "tablet.glb",
  phone: "phone.glb",
});

export const DEVICE_LAYER_ORDER = Object.freeze(["laptop", "phone", "tablet"]);
export const VISIBLE_OPACITY = 0.015;
const OPAQUE_OPACITY = 1 - Number.EPSILON;

function base(path) {
  return String(path).replace(/\/+$/, "");
}

export function modelUrl(assetBase, deviceName) {
  const file = MODEL_FILES[deviceName];
  if (!file) throw new Error(`unknown device ${deviceName}`);
  return `${base(assetBase)}/${file}`;
}

export function screenUrl(screenBase, textureName) {
  const name = String(textureName);
  return `${base(screenBase)}/${name.endsWith(".webp") ? name : `${name}.webp`}`;
}

// A capture that is not the display's native aspect is centred on neutral
// space rather than stretched across the glass.
export function getContainedTextureLayout(imageWidth, imageHeight, screenAspect) {
  const width = Math.max(1, Number(imageWidth) || 1);
  const height = Math.max(1, Number(imageHeight) || 1);
  const aspect = Math.max(0.01, Number(screenAspect) || width / height);
  const canvasWidth = width / height > aspect ? width : Math.ceil(height * aspect);
  const canvasHeight = width / height > aspect ? Math.ceil(width / aspect) : height;
  return {
    width: canvasWidth,
    height: canvasHeight,
    x: (canvasWidth - width) / 2,
    y: (canvasHeight - height) / 2,
  };
}

// One compile stall or one garbage collection is not what this machine costs
// per frame; the median of the samples is.
export function frameCostFromSamples(samples) {
  if (!samples.length) return 0;
  const sorted = [...samples].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function renderDeviceLayer(renderer, scene, camera, layer) {
  const { depthMaterial, restoreMaterial, model, index } = layer;
  model.root.visible = true;
  if (index > 0) {
    // Retain the previously composited device colour while giving this device
    // an independent depth buffer. A nearly invisible arrival can therefore
    // never cut its opaque silhouette out of another device.
    renderer.autoClear = false;
    renderer.clearDepth();
  }
  if (model.opacity < OPAQUE_OPACITY) {
    // Record only this device's nearest surface before its colour pass. Rear
    // shell, keyboard and hinge fragments then fail the depth test instead of
    // accumulating opacity through the foreground chassis.
    scene.overrideMaterial = depthMaterial;
    renderer.render(scene, camera);
    // The depth pass has already updated the shadow map for this frame.
    renderer.shadowMap.autoUpdate = false;
    scene.overrideMaterial = restoreMaterial;
    renderer.autoClear = false;
  }
  renderer.render(scene, camera);
  model.root.visible = false;
  renderer.shadowMap.autoUpdate = false;
}

// A fading device stays solid: it is drawn on its own depth buffer, depth
// first, colour second, in a fixed layer order.
export function renderSolidDeviceFades(renderer, scene, camera, depthMaterial, visibleModels) {
  const hasFadedModel = visibleModels.some(([, model]) => model.opacity < OPAQUE_OPACITY);
  // Keep multi-device frames layered at opacity 1 too. Switching back to a
  // combined depth buffer at the fade endpoint could reorder an overlap.
  if (!hasFadedModel && visibleModels.length < 2) {
    renderer.render(scene, camera);
    return;
  }
  const previousOverrideMaterial = scene.overrideMaterial;
  const previousAutoClear = renderer.autoClear;
  const previousShadowAutoUpdate = renderer.shadowMap.autoUpdate;
  const previousVisibility = visibleModels.map(([, model]) => model.root.visible);
  try {
    renderer.autoClear = true;
    visibleModels.forEach(([, model]) => { model.root.visible = false; });
    visibleModels.forEach(([, model], index) => {
      renderDeviceLayer(renderer, scene, camera, {
        depthMaterial,
        restoreMaterial: previousOverrideMaterial,
        model,
        index,
      });
    });
  } finally {
    visibleModels.forEach(([, model], index) => {
      model.root.visible = previousVisibility[index];
    });
    scene.overrideMaterial = previousOverrideMaterial;
    renderer.autoClear = previousAutoClear;
    renderer.shadowMap.autoUpdate = previousShadowAutoUpdate;
  }
}
