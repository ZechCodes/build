import { DEVICE_CONTRACT } from "./assets/devices/device-contract.js";
import { createDeviceEnvironment } from "./device-lighting.js";

const DEVICE_NAMES = ["laptop", "phone", "tablet"];
const MODEL_URLS = Object.freeze({
  laptop: "/landing/assets/devices/laptop-low.glb",
  phone: "/landing/assets/devices/phone.glb",
  tablet: "/landing/assets/devices/tablet.glb",
});
const SCREEN_DIRECTORY = "/landing/assets/screens";
const EMPTY_POSE = Object.freeze({ x: 50, y: 58, w: 0, rotate: [0, 0, 0], opacity: 0 });
const VISIBLE_OPACITY = 0.015;
const ARRIVAL_END = 0.45;
const DEPARTURE_START = 0.55;
const OPAQUE_OPACITY = 1 - Number.EPSILON;
const ENTRANCE_OFFSETS = Object.freeze({
  phone: Object.freeze({ x: 13, y: 10, scale: 0.72, rotate: [28, -8, 8] }),
  tablet: Object.freeze({ x: 18, y: 12, scale: 0.78, rotate: [26, -6, 6] }),
});

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

export function createDeviceScreenMaterial(three) {
  return new three.MeshBasicMaterial({
    color: 0xffffff,
    toneMapped: false,
  });
}

export function cloneDeviceSurfaceMaterial(material, maximumAnisotropy) {
  const clone = material.clone();
  if (clone.map) clone.map.anisotropy = Math.min(8, maximumAnisotropy);
  if (clone.name === "FrontGlass") {
    clone.metalness = 0;
    clone.roughness = 0.3;
    clone.specularIntensity = 0.18;
    clone.clearcoat = 0.1;
    clone.clearcoatRoughness = 0.3;
    clone.envMapIntensity = 0.3;
  }
  clone.transparent = true;
  return clone;
}

function deviceScreenAspect(deviceName) {
  const [width, height] = DEVICE_CONTRACT.devices[deviceName].screen.size_m;
  return width / height;
}

function deviceScreenToBoundsWidth(deviceName) {
  const device = DEVICE_CONTRACT.devices[deviceName];
  return device.bounds_size_m[0] / device.screen.size_m[0];
}

function deviceAnchor(deviceName) {
  const device = DEVICE_CONTRACT.devices[deviceName];
  if (deviceName === "laptop") {
    return [0, device.bounds_size_m[1] / 2, device.body_size_m[2] / 2];
  }
  // Put the display glass on the layout plane. Perspective then preserves the
  // exact review-panel alignment while the enclosure recedes behind it.
  return [0, 0, device.screen.center_m[2] || 0];
}

function clamp(value, minimum = 0, maximum = 1) {
  return Math.min(maximum, Math.max(minimum, Number(value) || 0));
}

function mix(from, to, amount) {
  return from + (to - from) * amount;
}

function smoothstep(value) {
  const bounded = clamp(value);
  return bounded * bounded * (3 - 2 * bounded);
}

function copyPose(pose = EMPTY_POSE) {
  const copy = {
    x: Number.isFinite(pose.x) ? pose.x : EMPTY_POSE.x,
    y: Number.isFinite(pose.y) ? pose.y : EMPTY_POSE.y,
    w: Number.isFinite(pose.w) ? pose.w : EMPTY_POSE.w,
    rotate: Array.isArray(pose.rotate) ? pose.rotate.slice(0, 3) : EMPTY_POSE.rotate.slice(),
    opacity: Number.isFinite(pose.opacity) ? pose.opacity : pose.w > 0 ? 1 : EMPTY_POSE.opacity,
  };
  if (Number.isFinite(pose.lidOpen)) copy.lidOpen = clamp(pose.lidOpen);
  if (Number.isFinite(pose.faceCamera)) copy.faceCamera = clamp(pose.faceCamera);
  return copy;
}

function interpolatePose(from, to, amount) {
  const start = copyPose(from);
  const end = copyPose(to);
  const eased = smoothstep(amount);
  const pose = {
    x: mix(start.x, end.x, eased),
    y: mix(start.y, end.y, eased),
    w: mix(start.w, end.w, eased),
    rotate: start.rotate.map((value, index) => mix(value, end.rotate[index] || 0, eased)),
    opacity: mix(start.opacity, end.opacity, eased),
  };
  if (Number.isFinite(start.lidOpen) || Number.isFinite(end.lidOpen)) {
    pose.lidOpen = mix(start.lidOpen ?? 1, end.lidOpen ?? 1, eased);
  }
  if (Number.isFinite(start.faceCamera) || Number.isFinite(end.faceCamera)) {
    pose.faceCamera = mix(start.faceCamera ?? 1, end.faceCamera ?? 1, eased);
  }
  return pose;
}

function profilePoses(scene, profile) {
  const poses = scene?.poses || {};
  return poses[profile] || poses.desktop || poses.tablet || poses.compact || {};
}

function scenePose(scenes, sceneIndex, profile, deviceName, stageAspect) {
  if (sceneIndex < 0 || sceneIndex >= scenes.length) return EMPTY_POSE;
  const settled = profilePoses(scenes[sceneIndex], profile)[deviceName] || EMPTY_POSE;
  if (
    sceneIndex !== 3
    || profile !== "tablet"
    || deviceName !== "tablet"
    || !Number.isFinite(stageAspect)
    || stageAspect <= 1
  ) {
    return settled;
  }
  const tablet = DEVICE_CONTRACT.devices.tablet;
  const widthForThirtyPercentHeight = 30
    * tablet.bounds_size_m[0] / tablet.bounds_size_m[1]
    / stageAspect;
  return { ...settled, y: 82, w: Math.min(settled.w, widthForThirtyPercentHeight) };
}

function initialPose(scenes, profile, deviceName, stageAspect) {
  const settled = copyPose(scenePose(scenes, 0, profile, deviceName, stageAspect));
  if (deviceName === "laptop") {
    // The hinge supplies the hero entrance: the base stays planted while the
    // display opens to the app rather than the whole laptop drifting upward.
    settled.lidOpen = 0;
  }
  return settled;
}

function entrancePose(settledPose, deviceName) {
  const settled = copyPose(settledPose);
  const offset = ENTRANCE_OFFSETS[deviceName];
  if (!offset) return settled;
  return {
    ...settled,
    x: settled.x + offset.x,
    y: settled.y + offset.y,
    w: settled.w * offset.scale,
    rotate: settled.rotate.map((value, index) => value + offset.rotate[index]),
    opacity: 0,
  };
}

function transitionPose(fromPose, toPose, amount, deviceName) {
  const from = copyPose(fromPose);
  const to = copyPose(toPose);
  const entering = from.opacity <= VISIBLE_OPACITY && to.opacity > VISIBLE_OPACITY;
  return interpolatePose(entering ? entrancePose(to, deviceName) : from, to, amount);
}

function reviewAlignmentPose(scenes, profile, reviewWidth) {
  const review = copyPose(profilePoses(scenes[4], profile).review);
  if (Number.isFinite(reviewWidth)) review.w = reviewWidth;
  review.w *= deviceScreenToBoundsWidth("tablet");
  review.rotate = [0, 0, 0];
  review.opacity = 1;
  // The tablet display must remain on the layout plane while it hands off to
  // the HTML review surface. Camera-facing compensation would tilt its plane.
  review.faceCamera = 0;
  return review;
}

function boundaryPose(scenes, beforeIndex, profile, deviceName, reviewWidth, stageAspect) {
  if (beforeIndex === 3 && deviceName === "tablet") {
    return reviewAlignmentPose(scenes, profile, reviewWidth);
  }
  return transitionPose(
    scenePose(scenes, beforeIndex, profile, deviceName, stageAspect),
    scenePose(scenes, beforeIndex + 1, profile, deviceName, stageAspect),
    0.5,
    deviceName,
  );
}

function reviewFramePose(scenes, local, profile, deviceName, reviewWidth, stageAspect) {
  const hidden = scenePose(scenes, 4, profile, deviceName, stageAspect);
  const arrival = deviceName === "tablet"
    ? reviewAlignmentPose(scenes, profile, reviewWidth)
    : boundaryPose(scenes, 3, profile, deviceName, reviewWidth, stageAspect);
  const departure = boundaryPose(scenes, 4, profile, deviceName, reviewWidth, stageAspect);
  if (local < 0.2) {
    if (deviceName !== "tablet") return interpolatePose(arrival, hidden, local / 0.2);
    return { ...arrival, opacity: mix(arrival.opacity, 0, smoothstep(local / 0.2)) };
  }
  if (local > 0.85) return interpolatePose(hidden, departure, (local - 0.85) / 0.15);
  return copyPose(hidden);
}

function framePose(scenes, sceneIndex, local, profile, deviceName, reviewWidth, stageAspect) {
  if (sceneIndex === 4) return reviewFramePose(scenes, local, profile, deviceName, reviewWidth, stageAspect);
  const settled = scenePose(scenes, sceneIndex, profile, deviceName, stageAspect);
  const arrival = sceneIndex === 0
    ? initialPose(scenes, profile, deviceName, stageAspect)
    : boundaryPose(scenes, sceneIndex - 1, profile, deviceName, reviewWidth, stageAspect);
  const departure = sceneIndex === scenes.length - 1
    ? settled
    : boundaryPose(scenes, sceneIndex, profile, deviceName, reviewWidth, stageAspect);
  if (local <= ARRIVAL_END) return interpolatePose(arrival, settled, local / ARRIVAL_END);
  if (local > DEPARTURE_START) {
    return interpolatePose(settled, departure, (local - DEPARTURE_START) / (1 - DEPARTURE_START));
  }
  return copyPose(settled);
}

export function getDeviceFramePoses(scenes, candidate, options = {}) {
  const frame = normalizedFrame(candidate, scenes);
  return Object.fromEntries(
    DEVICE_NAMES.map((name) => [
      name,
      framePose(
        scenes,
        frame.sceneIndex,
        frame.local,
        frame.profile,
        name,
        options.reviewWidth,
        options.stageAspect,
      ),
    ]),
  );
}

function normalizedFrame(candidate, scenes) {
  const state = candidate?.detail || candidate || {};
  const sceneIndex = Number.isInteger(state.sceneIndex)
    ? state.sceneIndex
    : Math.max(0, scenes.findIndex((scene) => scene === state.scene || scene.id === state.scene));
  return {
    ...state,
    sceneIndex,
    local: clamp(state.local),
    profile: state.profile || "desktop",
  };
}

function shouldEnhance(frame) {
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) return false;
  if (navigator.connection?.saveData) return false;
  if (matchMedia("(max-height: 599px)").matches) return false;
  if (frame?.profile === "static") return false;
  if (frame?.enhanced === false) return false;
  return true;
}

function deviceNamesForScene(sceneIndex) {
  if (sceneIndex >= 5) return DEVICE_NAMES;
  if (sceneIndex >= 3) return ["tablet"];
  if (sceneIndex >= 2) return DEVICE_NAMES;
  return ["laptop", "phone"];
}

export function getLaptopLidRotationDegrees(lidOpen = 1) {
  const laptop = DEVICE_CONTRACT.devices.laptop;
  return mix(
    laptop.lid_hinge_closed_rotation_deg ?? 90,
    laptop.lid_hinge_default_rotation_deg ?? -15,
    clamp(lidOpen),
  );
}

export function getCameraFacingRotation(pose, position, cameraPosition) {
  const [yaw = 0, pitch = 0, roll = 0] = pose.rotate;
  const amount = Number.isFinite(pose.faceCamera) ? clamp(pose.faceCamera) : 1;
  const cameraDistance = cameraPosition.z - position.z;
  return {
    pitch: -Math.atan2(cameraPosition.y - position.y, cameraDistance) * amount
      + pitch * Math.PI / 180,
    yaw: Math.atan2(-position.x, cameraDistance) * amount + yaw * Math.PI / 180,
    roll: roll * Math.PI / 180,
  };
}

export function getDeviceScreenSource(deviceName, frame) {
  const arriving = arrivingScreenSource(deviceName, frame);
  if (arriving) return arriving;
  const states = {
    0: { laptop: "ui01-macbook" },
    1: { laptop: "ui02-macbook", phone: "ui02-iphone" },
    3: { tablet: "ui04-ipad" },
    5: { laptop: "ui05-merged-macbook", tablet: "ui05-merged-ipad", phone: "ui05-merged-iphone" },
  };
  const state = frame.sceneIndex === 2
    ? directionScreenState(deviceName, frame.checkpoint)
    : frame.sceneIndex === 4
      ? reviewScreenState(deviceName, frame)
      : states[frame.sceneIndex]?.[deviceName];
  return state ? `${SCREEN_DIRECTORY}/${state}.webp` : null;
}

function arrivingScreenSource(deviceName, frame) {
  // Load an arriving device's first complete display before its pivot begins.
  if (frame.local < DEPARTURE_START) return null;
  if (frame.sceneIndex === 0 && deviceName === "phone") return `${SCREEN_DIRECTORY}/ui02-iphone.webp`;
  if (frame.sceneIndex === 2 && deviceName === "tablet") return `${SCREEN_DIRECTORY}/ui04-ipad.webp`;
  return null;
}

function directionScreenState(deviceName, checkpoint) {
  if (deviceName !== "phone") return null;
  const state = ["question", "answer", "resumed"].includes(checkpoint) ? checkpoint : "question";
  return `ui03-${state}-iphone`;
}

function reviewScreenState(deviceName, frame) {
  if (deviceName !== "tablet") return null;
  if (frame.checkpoint === "merged" || frame.local >= 0.8) return "ui05-merged-ipad";
  return frame.local < 0.2 ? "ui04-ipad" : "ui05-approval-ipad";
}

const WARMUP_FRAMES = 4;
const SLOW_FRAME_MS = 34;
const SLOW_FRAME_LIMIT = 4;
const REDUCED_QUALITY = 0.65;

// What a frame time means for the renderer: keep going, render at lower
// resolution, or hand the devices back to their posters. A slow renderer is a
// reason to stop drawing 3D, never a reason to take the story apart.
export function frameBudgetAction({ renderCount, slowFrameCount, qualityScale }, duration) {
  if (renderCount < WARMUP_FRAMES) return { action: "none", slowFrameCount, qualityScale };
  const slow = duration > SLOW_FRAME_MS ? slowFrameCount + 1 : 0;
  if (slow < SLOW_FRAME_LIMIT) return { action: "none", slowFrameCount: slow, qualityScale };
  if (qualityScale > REDUCED_QUALITY) return { action: "reduce", slowFrameCount: 0, qualityScale: REDUCED_QUALITY };
  return { action: "posters", slowFrameCount: slow, qualityScale };
}

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
      model.root.visible = true;
      if (index > 0) {
        // Retain the previously composited device color while giving this
        // device an independent depth buffer. A nearly invisible arrival can
        // therefore never cut its opaque silhouette out of another device.
        renderer.autoClear = false;
        renderer.clearDepth();
      }
      if (model.opacity < OPAQUE_OPACITY) {
        // Record only this device's nearest surface before its color pass.
        // Rear shell, keyboard, and hinge fragments then fail the depth test
        // instead of accumulating opacity through the foreground chassis.
        scene.overrideMaterial = depthMaterial;
        renderer.render(scene, camera);
        // The depth pass has already updated the shadow map for this frame.
        renderer.shadowMap.autoUpdate = false;
        scene.overrideMaterial = previousOverrideMaterial;
        renderer.autoClear = false;
      }
      renderer.render(scene, camera);
      model.root.visible = false;
      // An opaque first layer also completes the frame's shadow update.
      renderer.shadowMap.autoUpdate = false;
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

function idle(callback) {
  if ("requestIdleCallback" in window) return requestIdleCallback(callback, { timeout: 1600 });
  return setTimeout(callback, 32);
}

function cancelIdle(handle) {
  if ("cancelIdleCallback" in window) cancelIdleCallback(handle);
  else clearTimeout(handle);
}

class DeviceStage {
  constructor(stage, options) {
    this.stage = stage;
    this.page = stage.ownerDocument;
    this.eventTarget = options.eventTarget || stage.closest("[data-story-stage]") || stage;
    this.reviewSurface = this.eventTarget.querySelector("[data-review-surface]");
    this.options = options;
    this.scenes = options.scenes || [];
    this.story = options.story || window.BuildLandingStory;
    this.frame = normalizedFrame(this.story?.getState?.() || {}, this.scenes);
    this.models = new Map();
    this.modelPromises = new Map();
    this.textures = new Map();
    this.texturePromises = new Map();
    this.posterElements = new Map();
    this.posterOpacity = new Map();
    this.enabled = false;
    this.initializing = false;
    this.failed = false;
    this.destroyed = false;
    this.visible = true;
    this.renderPending = false;
    this.renderCount = 0;
    this.slowFrameCount = 0;
    this.qualityScale = 1;
    this.loadQueue = Promise.resolve();
    this.disposers = [];
    this.capturePosters();
    this.listen();
    if (shouldEnhance(this.frame)) this.scheduleInitialize();
  }

  capturePosters() {
    for (const name of DEVICE_NAMES) {
      const element = this.stage.querySelector(`[data-device="${name}"]`);
      if (!element) continue;
      if (!element.getAttribute("src") && element.dataset.src) {
        element.setAttribute("src", element.dataset.src);
      }
      this.posterElements.set(name, element);
      this.posterOpacity.set(name, {
        value: element.style.getPropertyValue("opacity"),
        priority: element.style.getPropertyPriority("opacity"),
        transition: element.style.transition,
      });
    }
  }

  listen() {
    const onStoryFrame = (event) => this.update(event);
    const onVisibility = () => {
      if (!this.page.hidden) this.requestRender();
    };
    const onMotionPreference = (event) => {
      if (event.matches) this.suspend();
      else this.update(this.story?.getState?.() || this.frame);
    };
    const motionQuery = matchMedia("(prefers-reduced-motion: reduce)");
    this.eventTarget.addEventListener("build:storyframe", onStoryFrame);
    this.page.addEventListener("visibilitychange", onVisibility);
    motionQuery.addEventListener("change", onMotionPreference);
    this.disposers.push(() => this.eventTarget.removeEventListener("build:storyframe", onStoryFrame));
    this.disposers.push(() => this.page.removeEventListener("visibilitychange", onVisibility));
    this.disposers.push(() => motionQuery.removeEventListener("change", onMotionPreference));
    const unsubscribe = this.story?.subscribe?.((state) => this.update(state));
    if (typeof unsubscribe === "function") this.disposers.push(unsubscribe);
    if ("IntersectionObserver" in window) {
      this.intersectionObserver = new IntersectionObserver(
        ([entry]) => {
          this.visible = entry.isIntersecting;
          if (this.visible) this.requestRender();
        },
        { rootMargin: "20% 0px" },
      );
      this.intersectionObserver.observe(this.stage);
      this.disposers.push(() => this.intersectionObserver.disconnect());
    }
  }

  scheduleInitialize() {
    if (this.idleHandle !== undefined || this.initializing || this.failed || this.destroyed) return;
    this.idleHandle = idle(() => {
      this.idleHandle = undefined;
      this.initialize();
    });
  }

  async initialize() {
    if (this.destroyed || this.failed || this.initializing || !shouldEnhance(this.frame)) return;
    if (this.renderer) {
      this.enabled = true;
      this.prefetchForFrame();
      this.requestRender();
      return;
    }
    this.initializing = true;
    try {
      this.three = await import("./vendor/three-device-runtime.js");
      if (this.destroyed) return;
      this.createScene();
      this.enabled = true;
      await this.ensureModel("laptop");
      this.prefetchForFrame();
    } catch (error) {
      this.fail(error);
    } finally {
      this.initializing = false;
    }
  }

  createScene() {
    const {
      ACESFilmicToneMapping,
      Color,
      DirectionalLight,
      HemisphereLight,
      PCFSoftShadowMap,
      PerspectiveCamera,
      Scene,
      SRGBColorSpace,
      WebGLRenderer,
    } = this.three;
    this.canvas = this.page.createElement("canvas");
    this.canvas.className = "device-stage__canvas";
    this.canvas.setAttribute("aria-hidden", "true");
    this.canvas.style.cssText = "position:absolute;inset:0;width:100%;height:100%;pointer-events:none;opacity:0;transition:opacity 320ms ease;z-index:2";
    this.stage.append(this.canvas);
    this.renderer = new WebGLRenderer({ canvas: this.canvas, alpha: true, antialias: true, powerPreference: "high-performance" });
    this.renderer.setClearColor(new Color(0x000000), 0);
    this.renderer.outputColorSpace = SRGBColorSpace;
    this.renderer.toneMapping = ACESFilmicToneMapping;
    this.renderer.toneMappingExposure = 1.04;
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = PCFSoftShadowMap;
    this.solidFadeDepthMaterial = new this.three.MeshBasicMaterial();
    this.solidFadeDepthMaterial.colorWrite = false;
    this.solidFadeDepthMaterial.depthWrite = true;
    this.solidFadeDepthMaterial.depthTest = true;
    if (this.isSoftwareRenderer()) throw new Error("software WebGL renderer");
    this.scene = new Scene();
    this.environmentTarget = createDeviceEnvironment(this.three, this.renderer);
    this.scene.environment = this.environmentTarget.texture;
    this.scene.environmentIntensity = 1;
    // Keep one vertical world unit at the layout plane, while giving the
    // hardware photographic convergence and spatially varying reflections.
    this.camera = new PerspectiveCamera(20, 1, 0.5, 8);
    this.camera.position.set(0, 0, 0.5 / Math.tan(Math.PI / 18));
    this.camera.lookAt(0, 0, 0);
    const sky = new HemisphereLight(0xf1f3f2, 0x121820, 0.12);
    const key = new DirectionalLight(0xfffcf6, 1.8);
    const fill = new DirectionalLight(0xe8edf5, 0.35);
    const edge = new DirectionalLight(0xf2f4f3, 1.1);
    key.position.set(-4, 5, 6);
    fill.position.set(5, 1, 4);
    edge.position.set(2, 4, -5);
    key.castShadow = true;
    key.shadow.mapSize.set(2048, 2048);
    key.shadow.bias = -0.000006;
    key.shadow.normalBias = 0.000018;
    this.keyLight = key;
    this.scene.add(sky, key, key.target, fill, edge);
    this.loader = new this.three.GLTFLoader();
    this.textureLoader = new this.three.TextureLoader();
    const onResize = () => {
      this.resize();
      this.requestRender();
    };
    this.resizeObserver = new ResizeObserver(onResize);
    this.resizeObserver.observe(this.stage);
    this.disposers.push(() => this.resizeObserver.disconnect());
    this.canvas.addEventListener("webglcontextlost", (event) => {
      event.preventDefault();
      this.fail(new Error("WebGL context lost"));
    }, { once: true });
    this.resize();
  }

  isSoftwareRenderer() {
    const context = this.renderer.getContext();
    const extension = context.getExtension("WEBGL_debug_renderer_info");
    const rendererName = extension
      ? context.getParameter(extension.UNMASKED_RENDERER_WEBGL)
      : context.getParameter(context.RENDERER);
    return /swiftshader|llvmpipe|software rasterizer/i.test(String(rendererName));
  }

  resize() {
    if (!this.renderer) return;
    const { width, height } = this.stage.getBoundingClientRect();
    if (width < 1 || height < 1) return;
    // 1.5x is enough for a device render behind copy; 2x doubles the fill
    // cost on every scroll frame.
    this.pixelRatio = Math.min(devicePixelRatio || 1, 1.5) * this.qualityScale;
    this.reviewWidthPercent = this.measureReviewWidth();
    this.renderer.setPixelRatio(this.pixelRatio);
    this.renderer.setSize(width, height, false);
    const aspect = width / height;
    this.camera.aspect = aspect;
    this.camera.updateProjectionMatrix();
    this.aspect = aspect;
  }

  update(candidate) {
    this.frame = normalizedFrame(candidate, this.scenes);
    if (!shouldEnhance(this.frame)) {
      this.suspend();
      return;
    }
    if (!this.enabled && this.renderer && !this.failed) this.enabled = true;
    if (!this.enabled) this.scheduleInitialize();
    this.prefetchForFrame();
    this.updateScreens();
    this.requestRender();
  }

  prefetchForFrame() {
    if (!this.enabled) return;
    for (const name of deviceNamesForScene(this.frame.sceneIndex)) this.ensureModel(name);
  }

  ensureModel(name) {
    if (this.models.has(name)) return Promise.resolve(this.models.get(name));
    if (this.modelPromises.has(name)) return this.modelPromises.get(name);
    const modelPromise = this.loadQueue.then(() => this.loadModel(name));
    this.loadQueue = modelPromise.catch(() => undefined);
    this.modelPromises.set(name, modelPromise);
    modelPromise.catch((error) => this.fail(error));
    return modelPromise;
  }

  async loadModel(name) {
    const url = this.options.modelUrls?.[name] || MODEL_URLS[name];
    const gltf = await this.loader.loadAsync(url);
    if (this.destroyed) {
      gltf.scene.traverse((node) => {
        node.geometry?.dispose();
        const materials = Array.isArray(node.material) ? node.material : [node.material];
        for (const material of materials) material?.dispose?.();
      });
      return undefined;
    }
    const model = this.prepareModel(name, gltf.scene);
    this.models.set(name, model);
    this.modelPromises.delete(name);
    this.scene.add(model.root);
    this.updateScreens();
    this.requestRender();
    return model;
  }

  prepareModel(name, source) {
    const { Group, Mesh } = this.three;
    source.updateMatrixWorld(true);
    const [anchorX, anchorY, anchorZ] = deviceAnchor(name);
    source.position.set(-anchorX, -anchorY, -anchorZ);
    const screenMeshes = [];
    source.traverse((node) => {
      if (!(node instanceof Mesh)) return;
      node.castShadow = !/screen|legend/.test(node.name);
      node.receiveShadow = node.castShadow;
      if (node.name === DEVICE_CONTRACT.devices[name].screen.node) {
        node.material = createDeviceScreenMaterial(this.three);
        node.material.transparent = true;
        node.userData.deviceScreen = name;
        screenMeshes.push(node);
        return;
      }
      const materials = Array.isArray(node.material) ? node.material : [node.material];
      const maximumAnisotropy = this.renderer.capabilities.getMaxAnisotropy();
      const clones = materials.map((material) => (
        cloneDeviceSurfaceMaterial(material, maximumAnisotropy)
      ));
      node.material = Array.isArray(node.material) ? clones : clones[0];
    });
    if (!screenMeshes.length) throw new Error(`Missing ${name} screen mesh`);
    const hingeNodeName = DEVICE_CONTRACT.devices[name].lid_hinge_node;
    const lidHinge = hingeNodeName ? source.getObjectByName(hingeNodeName) : null;
    if (hingeNodeName && !lidHinge) throw new Error(`Missing ${name} lid hinge node`);
    const root = new Group();
    root.name = `device-${name}`;
    root.add(source);
    root.visible = false;
    return {
      root,
      source,
      screenMeshes,
      lidHinge,
      screenUrl: null,
      screenKey: null,
      desiredScreenUrl: null,
      width: DEVICE_CONTRACT.devices[name].bounds_size_m[0],
      ready: false,
    };
  }

  updateScreens() {
    for (const [name, model] of this.models) {
      const url = getDeviceScreenSource(name, this.frame);
      if (url === model.desiredScreenUrl) continue;
      model.desiredScreenUrl = url;
      // Keep the current complete display during a checkpoint load or a
      // departing pose. The system bars must never blink to an empty screen.
      if (!url) {
        this.releaseUnusedTextures();
        continue;
      }
      if (!model.screenUrl) this.showPoster(name);
      this.assignScreenTexture(name, model, url);
    }
  }

  async assignScreenTexture(name, model, url) {
    try {
      const texture = await this.loadTexture(url, name);
      if (this.destroyed || this.failed || model.desiredScreenUrl !== url) {
        this.releaseUnusedTextures();
        return;
      }
      for (const screen of model.screenMeshes) {
        screen.material.map = texture;
        screen.material.color.setHex(0xffffff);
        screen.material.needsUpdate = true;
      }
      model.screenUrl = url;
      model.screenKey = `${name}:${url}`;
      this.releaseUnusedTextures();
      this.requestRender();
    } catch (error) {
      this.fail(error);
    }
  }

  loadTexture(url, deviceName) {
    const key = `${deviceName}:${url}`;
    if (this.textures.has(key)) return Promise.resolve(this.textures.get(key));
    if (this.texturePromises.has(key)) return this.texturePromises.get(key);
    const promise = this.textureLoader.loadAsync(url).then((sourceTexture) => {
      if (this.destroyed || this.failed) {
        sourceTexture.dispose();
        throw new Error("device stage no longer accepts textures");
      }
      const texture = this.containScreenTexture(sourceTexture, deviceName);
      texture.colorSpace = this.three.SRGBColorSpace;
      // Preserve small system glyphs and key legends as the display turns.
      texture.anisotropy = Math.min(8, this.renderer.capabilities.getMaxAnisotropy());
      texture.flipY = false;
      texture.needsUpdate = true;
      this.textures.set(key, texture);
      return texture;
    }).finally(() => this.texturePromises.delete(key));
    this.texturePromises.set(key, promise);
    return promise;
  }

  containScreenTexture(source, deviceName) {
    const image = source.image;
    const width = image.naturalWidth || image.width;
    const height = image.naturalHeight || image.height;
    const layout = getContainedTextureLayout(width, height, deviceScreenAspect(deviceName));
    if (layout.width === width && layout.height === height) return source;
    const canvas = this.page.createElement("canvas");
    canvas.width = layout.width;
    canvas.height = layout.height;
    const context = canvas.getContext("2d");
    if (!context) return source;
    context.fillStyle = "#07110f";
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.drawImage(image, layout.x, layout.y, width, height);
    const texture = new this.three.CanvasTexture(canvas);
    source.dispose();
    return texture;
  }

  releaseUnusedTextures() {
    const desired = new Set(
      Array.from(this.models, ([name, model]) => [
        model.desiredScreenUrl ? `${name}:${model.desiredScreenUrl}` : null,
        model.screenKey,
      ]).flat(),
    );
    for (const [key, texture] of this.textures) {
      if (desired.has(key)) continue;
      texture.dispose();
      this.textures.delete(key);
    }
  }

  applyPose(model, pose) {
    const opacity = clamp(pose.opacity);
    model.opacity = opacity;
    model.root.visible = opacity > VISIBLE_OPACITY && pose.w > 0 && Boolean(model.screenUrl);
    if (!model.root.visible) return;
    const worldWidth = (pose.w / 100) * this.aspect;
    const scale = worldWidth / model.width;
    model.root.scale.setScalar(scale);
    model.root.position.set(
      ((pose.x / 100) - 0.5) * this.aspect,
      0.5 - (pose.y / 100),
      0,
    );
    const rotation = getCameraFacingRotation(pose, model.root.position, this.camera.position);
    model.root.rotation.set(
      rotation.pitch,
      rotation.yaw,
      rotation.roll,
      "YXZ",
    );
    if (model.lidHinge) {
      model.lidHinge.rotation.x = this.three.MathUtils.degToRad(
        getLaptopLidRotationDegrees(pose.lidOpen),
      );
    }
    model.source.traverse((node) => {
      if (!node.isMesh) return;
      const materials = Array.isArray(node.material) ? node.material : [node.material];
      for (const material of materials) {
        material.opacity = opacity;
        // Depth writes stay on while fading: without them the keyboard and
        // hinge draw through the lid and the laptop reads as a grey silhouette.
        material.depthWrite = true;
      }
    });
  }

  requestRender() {
    if (!this.enabled || this.destroyed || this.renderPending || !this.visible || this.page.hidden) return;
    this.renderPending = true;
    requestAnimationFrame(() => {
      this.renderPending = false;
      this.render();
    });
  }

  render() {
    if (!this.canRender()) return;
    const startedAt = performance.now();
    const poses = getDeviceFramePoses(this.scenes, this.frame, {
      reviewWidth: this.reviewWidth(),
      stageAspect: this.aspect,
    });
    const visibleModels = this.poseModels(poses);
    this.fitKeyboardShadow();
    renderSolidDeviceFades(
      this.renderer,
      this.scene,
      this.camera,
      this.solidFadeDepthMaterial,
      visibleModels,
    );
    this.renderCount += 1;
    this.publishFrame(visibleModels);
    this.measureFrame(performance.now() - startedAt);
  }

  canRender() {
    return this.enabled && this.visible && !this.page.hidden && Boolean(this.aspect);
  }

  fitKeyboardShadow() {
    const laptop = this.models.get("laptop");
    this.keyLight.castShadow = Boolean(laptop?.root.visible);
    if (!this.keyLight.castShadow) return;
    const center = laptop.root.position;
    const span = laptop.width * laptop.root.scale.x * 0.72;
    this.keyLight.target.position.copy(center);
    this.keyLight.position.copy(center).add({ x: -2, y: 3, z: 4 });
    const shadowCamera = this.keyLight.shadow.camera;
    Object.assign(shadowCamera, {
      left: -span, right: span, top: span, bottom: -span,
      near: Math.sqrt(29) - span * 2, far: Math.sqrt(29) + span * 2,
    });
    shadowCamera.updateProjectionMatrix();
  }

  reviewWidth() {
    if (this.reviewWidthPercent === undefined) this.reviewWidthPercent = this.measureReviewWidth();
    return this.reviewWidthPercent;
  }

  // Layout reads are done on resize, not on every scroll frame.
  measureReviewWidth() {
    const stageWidth = this.eventTarget.getBoundingClientRect().width;
    if (stageWidth <= 0 || !this.reviewSurface) return undefined;
    return this.reviewSurface.getBoundingClientRect().width / stageWidth * 100;
  }

  poseModels(poses) {
    const visibleModels = [];
    for (const name of DEVICE_NAMES) {
      const model = this.models.get(name);
      if (!model) continue;
      const pose = poses[name];
      this.applyPose(model, pose);
      if (model.root.visible) visibleModels.push([name, model]);
    }
    return visibleModels;
  }

  publishFrame(visibleModels) {
    if (!visibleModels.length) return;
    this.canvas.style.opacity = "1";
    this.stage.classList.add("device-stage--enhanced");
    this.stage.dataset.webglReady = "true";
    for (const [name, model] of visibleModels) {
      if (!model.ready) {
        if (!model.desiredScreenUrl || model.screenUrl === model.desiredScreenUrl) {
          this.revealModel(name, model);
        }
      } else {
        this.hidePoster(name);
      }
    }
  }

  revealModel(name, model) {
    model.ready = true;
    this.hidePoster(name);
  }

  hidePoster(name) {
    const poster = this.posterElements.get(name);
    if (!poster) return;
    poster.style.transition = "opacity 320ms ease";
    poster.style.setProperty("opacity", "0", "important");
    poster.dataset.webglDeviceReady = "true";
    poster.dataset.webglScreenSource = this.models.get(name)?.screenUrl || "";
  }

  showPoster(name) {
    const poster = this.posterElements.get(name);
    if (!poster) return;
    const original = this.posterOpacity.get(name);
    poster.style.setProperty("opacity", original.value, original.priority);
    poster.removeAttribute("data-webgl-device-ready");
    poster.removeAttribute("data-webgl-screen-source");
  }

  measureFrame(duration) {
    this.lastFrameMs = duration;
    const next = frameBudgetAction(this, duration);
    this.slowFrameCount = next.slowFrameCount;
    if (next.action === "reduce") {
      this.qualityScale = next.qualityScale;
      this.resize();
    } else if (next.action === "posters") {
      this.destroy();
    }
  }

  restorePosters() {
    for (const [name, element] of this.posterElements) {
      const original = this.posterOpacity.get(name);
      element.style.setProperty("opacity", original.value, original.priority);
      element.style.transition = original.transition;
      element.removeAttribute("data-webgl-device-ready");
      element.removeAttribute("data-webgl-screen-source");
    }
    this.stage.classList.remove("device-stage--enhanced");
    delete this.stage.dataset.webglReady;
  }

  fail(error) {
    if (this.failed || this.destroyed) return;
    this.failed = true;
    this.options.onFailure?.(error);
    this.destroy();
  }

  suspend() {
    this.enabled = false;
    this.restorePosters();
    if (this.canvas) this.canvas.style.opacity = "0";
  }

  disposeGraphics() {
    this.environmentTarget?.dispose();
    this.solidFadeDepthMaterial?.dispose();
    this.renderer?.dispose();
    this.renderer?.forceContextLoss();
    this.canvas?.remove();
  }

  destroy() {
    if (this.destroyed) return;
    this.destroyed = true;
    this.enabled = false;
    if (this.idleHandle !== undefined) cancelIdle(this.idleHandle);
    for (const dispose of this.disposers.splice(0)) dispose();
    for (const model of this.models.values()) {
      model.desiredScreenUrl = null;
      model.screenUrl = null;
      model.screenKey = null;
      const { root } = model;
      root.traverse((node) => {
        if (!node.isMesh) return;
        node.geometry?.dispose();
        const materials = Array.isArray(node.material) ? node.material : [node.material];
        for (const material of materials) material.dispose();
      });
    }
    this.models.clear();
    for (const texture of this.textures.values()) texture.dispose();
    this.textures.clear();
    this.disposeGraphics();
    this.restorePosters();
  }
}

export function installDeviceStage(stage, options = {}) {
  if (!stage) return { destroy() {}, refresh() {}, getState: () => ({ enabled: false }) };
  const deviceStage = new DeviceStage(stage, options);
  return {
    destroy: () => deviceStage.destroy(),
    refresh: () => deviceStage.update(deviceStage.story?.getState?.() || deviceStage.frame),
    getState: () => ({
      enabled: deviceStage.enabled,
      failed: deviceStage.failed,
      destroyed: deviceStage.destroyed,
      loadedDevices: Array.from(deviceStage.models.keys()),
      pixelRatio: deviceStage.pixelRatio || 0,
      renderCount: deviceStage.renderCount,
      lastFrameMs: deviceStage.lastFrameMs || 0,
    }),
  };
}
