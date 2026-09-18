import { DEVICE_CONTRACT } from "./assets/devices/device-contract.js";

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

function deviceScreenAspect(deviceName) {
  const [width, height] = DEVICE_CONTRACT.devices[deviceName].screen.size_m;
  return width / height;
}

function deviceScreenToBoundsWidth(deviceName) {
  const device = DEVICE_CONTRACT.devices[deviceName];
  return device.bounds_size_m[0] / device.screen.size_m[0];
}

function deviceAnchor(deviceName) {
  if (deviceName !== "laptop") return [0, 0, 0];
  const device = DEVICE_CONTRACT.devices.laptop;
  return [0, device.bounds_size_m[1] / 2, device.body_size_m[2] / 2];
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
  return {
    x: Number.isFinite(pose.x) ? pose.x : EMPTY_POSE.x,
    y: Number.isFinite(pose.y) ? pose.y : EMPTY_POSE.y,
    w: Number.isFinite(pose.w) ? pose.w : EMPTY_POSE.w,
    rotate: Array.isArray(pose.rotate) ? pose.rotate.slice(0, 3) : EMPTY_POSE.rotate.slice(),
    opacity: Number.isFinite(pose.opacity) ? pose.opacity : pose.w > 0 ? 1 : EMPTY_POSE.opacity,
  };
}

function interpolatePose(from, to, amount) {
  const start = copyPose(from);
  const end = copyPose(to);
  const eased = smoothstep(amount);
  return {
    x: mix(start.x, end.x, eased),
    y: mix(start.y, end.y, eased),
    w: mix(start.w, end.w, eased),
    rotate: start.rotate.map((value, index) => mix(value, end.rotate[index] || 0, eased)),
    opacity: mix(start.opacity, end.opacity, eased),
  };
}

function profilePoses(scene, profile) {
  const poses = scene?.poses || {};
  return poses[profile] || poses.desktop || poses.tablet || poses.compact || {};
}

function scenePose(scenes, sceneIndex, profile, deviceName) {
  if (sceneIndex < 0 || sceneIndex >= scenes.length) return EMPTY_POSE;
  return profilePoses(scenes[sceneIndex], profile)[deviceName] || EMPTY_POSE;
}

function initialPose(scenes, profile, deviceName) {
  const settled = copyPose(scenePose(scenes, 0, profile, deviceName));
  if (deviceName === "laptop") settled.w = Math.max(0, settled.w - 5);
  return settled;
}

function reviewAlignmentPose(scenes, profile, reviewWidth) {
  const review = copyPose(profilePoses(scenes[4], profile).review);
  if (Number.isFinite(reviewWidth)) review.w = reviewWidth;
  review.w *= deviceScreenToBoundsWidth("tablet");
  review.rotate = [0, 0, 0];
  review.opacity = 1;
  return review;
}

function boundaryPose(scenes, beforeIndex, profile, deviceName, reviewWidth) {
  if (beforeIndex === 3 && deviceName === "tablet") {
    return reviewAlignmentPose(scenes, profile, reviewWidth);
  }
  return interpolatePose(
    scenePose(scenes, beforeIndex, profile, deviceName),
    scenePose(scenes, beforeIndex + 1, profile, deviceName),
    0.5,
  );
}

function reviewFramePose(scenes, local, profile, deviceName, reviewWidth) {
  const hidden = scenePose(scenes, 4, profile, deviceName);
  const arrival = deviceName === "tablet"
    ? reviewAlignmentPose(scenes, profile, reviewWidth)
    : boundaryPose(scenes, 3, profile, deviceName, reviewWidth);
  const departure = boundaryPose(scenes, 4, profile, deviceName, reviewWidth);
  if (local < 0.2) {
    if (deviceName !== "tablet") return interpolatePose(arrival, hidden, local / 0.2);
    return { ...arrival, opacity: mix(arrival.opacity, 0, smoothstep(local / 0.2)) };
  }
  if (local > 0.85) return interpolatePose(hidden, departure, (local - 0.85) / 0.15);
  return copyPose(hidden);
}

function framePose(scenes, sceneIndex, local, profile, deviceName, reviewWidth) {
  if (sceneIndex === 4) return reviewFramePose(scenes, local, profile, deviceName, reviewWidth);
  const settled = scenePose(scenes, sceneIndex, profile, deviceName);
  const arrival = sceneIndex === 0
    ? initialPose(scenes, profile, deviceName)
    : boundaryPose(scenes, sceneIndex - 1, profile, deviceName, reviewWidth);
  const departure = sceneIndex === scenes.length - 1
    ? settled
    : boundaryPose(scenes, sceneIndex, profile, deviceName, reviewWidth);
  if (local < ARRIVAL_END) return interpolatePose(arrival, settled, local / ARRIVAL_END);
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
      framePose(scenes, frame.sceneIndex, frame.local, frame.profile, name, options.reviewWidth),
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
  if (sceneIndex >= 1) return ["laptop", "phone"];
  return ["laptop"];
}

export function getDeviceScreenSource(deviceName, frame) {
  const states = {
    0: { laptop: "ui01-desktop" },
    1: { laptop: "ui02-desktop", phone: "ui02-mobile" },
    3: { tablet: "ui04-tablet" },
    5: { laptop: "ui05-merged-desktop", tablet: "ui05-merged-tablet", phone: "ui05-merged-mobile" },
  };
  const state = frame.sceneIndex === 2
    ? directionScreenState(deviceName, frame.checkpoint)
    : frame.sceneIndex === 4
      ? reviewScreenState(deviceName, frame)
      : states[frame.sceneIndex]?.[deviceName];
  return state ? `${SCREEN_DIRECTORY}/${state}.webp` : null;
}

function directionScreenState(deviceName, checkpoint) {
  if (deviceName !== "phone") return null;
  const state = ["question", "answer", "resumed"].includes(checkpoint) ? checkpoint : "question";
  return `ui03-${state}-mobile`;
}

function reviewScreenState(deviceName, frame) {
  if (deviceName !== "tablet") return null;
  if (frame.checkpoint === "merged" || frame.local >= 0.8) return "ui05-merged-tablet";
  return frame.local < 0.2 ? "ui04-tablet" : "ui05-approval-tablet";
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
      OrthographicCamera,
      PMREMGenerator,
      RoomEnvironment,
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
    if (this.isSoftwareRenderer()) throw new Error("software WebGL renderer");
    this.scene = new Scene();
    const room = new RoomEnvironment();
    const environmentGenerator = new PMREMGenerator(this.renderer);
    this.environmentTarget = environmentGenerator.fromScene(room, 0.04);
    this.scene.environment = this.environmentTarget.texture;
    this.scene.environmentIntensity = 0.46;
    room.dispose();
    environmentGenerator.dispose();
    this.camera = new OrthographicCamera(-1, 1, 1, -1, 0.01, 20);
    this.camera.position.set(0, 0, 6);
    this.camera.lookAt(0, 0, 0);
    const sky = new HemisphereLight(0xf1f3f2, 0x202322, 1);
    const key = new DirectionalLight(0xfffaf2, 1.9);
    const fill = new DirectionalLight(0xdde2e1, 0.85);
    const edge = new DirectionalLight(0xf2f4f3, 0.7);
    key.position.set(-4, 5, 6);
    fill.position.set(5, 1, 4);
    edge.position.set(2, 4, -5);
    this.scene.add(sky, key, fill, edge);
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
    this.camera.left = -aspect / 2;
    this.camera.right = aspect / 2;
    this.camera.top = 0.5;
    this.camera.bottom = -0.5;
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
    const { Group, Mesh, MeshBasicMaterial } = this.three;
    source.updateMatrixWorld(true);
    const [anchorX, anchorY, anchorZ] = deviceAnchor(name);
    source.position.set(-anchorX, -anchorY, -anchorZ);
    const screenMeshes = [];
    source.traverse((node) => {
      if (!(node instanceof Mesh)) return;
      if (node.name === DEVICE_CONTRACT.devices[name].screen.node) {
        node.material = new MeshBasicMaterial({ color: 0x0b100e, toneMapped: false });
        node.material.transparent = true;
        node.userData.deviceScreen = name;
        screenMeshes.push(node);
        return;
      }
      const materials = Array.isArray(node.material) ? node.material : [node.material];
      const clones = materials.map((material) => {
        const clone = material.clone();
        clone.transparent = true;
        return clone;
      });
      node.material = Array.isArray(node.material) ? clones : clones[0];
    });
    if (!screenMeshes.length) throw new Error(`Missing ${name} screen mesh`);
    const root = new Group();
    root.name = `device-${name}`;
    root.add(source);
    root.visible = false;
    return {
      root,
      source,
      screenMeshes,
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
      model.screenUrl = null;
      model.screenKey = null;
      model.ready = false;
      this.showPoster(name);
      for (const screen of model.screenMeshes) {
        screen.material.map = null;
        screen.material.color.setHex(0x0b100e);
        screen.material.needsUpdate = true;
      }
      if (!url) {
        this.releaseUnusedTextures();
        continue;
      }
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
    model.root.visible = opacity > VISIBLE_OPACITY && pose.w > 0;
    if (!model.root.visible) return;
    const worldWidth = (pose.w / 100) * this.aspect;
    const scale = worldWidth / model.width;
    model.root.scale.setScalar(scale);
    model.root.position.set(
      ((pose.x / 100) - 0.5) * this.aspect,
      0.5 - (pose.y / 100),
      0,
    );
    const [yaw = 0, pitch = 0, roll = 0] = pose.rotate;
    model.root.rotation.set(
      this.three.MathUtils.degToRad(pitch),
      this.three.MathUtils.degToRad(yaw),
      this.three.MathUtils.degToRad(roll),
      "YXZ",
    );
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
    });
    const visibleModels = this.poseModels(poses);
    this.renderer.render(this.scene, this.camera);
    this.renderCount += 1;
    this.publishFrame(visibleModels);
    this.measureFrame(performance.now() - startedAt);
  }

  canRender() {
    return this.enabled && this.visible && !this.page.hidden && Boolean(this.aspect);
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
  }

  showPoster(name) {
    const poster = this.posterElements.get(name);
    if (!poster) return;
    const original = this.posterOpacity.get(name);
    poster.style.setProperty("opacity", original.value, original.priority);
    poster.removeAttribute("data-webgl-device-ready");
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
    this.environmentTarget?.dispose();
    this.renderer?.dispose();
    this.renderer?.forceContextLoss();
    this.canvas?.remove();
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
