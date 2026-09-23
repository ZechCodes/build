// The device stage: three original GLB devices on a studio-lit perspective
// stage. It knows nothing about scenes, scroll or the DOM beyond its canvas.
// The choreography calls setPose/setScreen from its timelines.
import {
  ACESFilmicToneMapping,
  CanvasTexture,
  Color,
  DirectionalLight,
  Group,
  HemisphereLight,
  Mesh,
  MeshBasicMaterial,
  PCFShadowMap,
  PMREMGenerator,
  PerspectiveCamera,
  PlaneGeometry,
  Scene,
  SRGBColorSpace,
  TextureLoader,
  WebGLRenderer,
} from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { DEVICE_CONTRACT } from "./device-contract.js";
import {
  STAGE_CAMERA,
  deviceAnchor,
  deviceScreenAspect,
  normalizePose,
  poseToWorld,
  screenCornersPx,
} from "./pose.js";
import {
  DEVICE_LAYER_ORDER,
  VISIBLE_OPACITY,
  frameCostFromSamples,
  getContainedTextureLayout,
  modelUrl,
  renderSolidDeviceFades,
  screenUrl,
} from "./render.js";

const MAX_PIXEL_RATIO = 1.5;
const MAX_ANISOTROPY = 8;
const LETTERBOX_COLOR = "#07110f";

// Reflected studio strips shape the aluminium, with dark space between them.
// This scene is baked once; the light cards never appear on the page.
function createDeviceEnvironment(renderer) {
  const studio = new Scene();
  studio.background = new Color(0x111316);
  const cards = [
    { size: [3.5, 5], position: [-4, 3, 5], color: 0xfffcf6, intensity: 4.0 },
    { size: [0.85, 5], position: [4, 1, 3], color: 0xf1f4fa, intensity: 1.7 },
    { size: [5, 1.1], position: [0, 5, 1], color: 0xffffff, intensity: 3.2 },
    { size: [6, 2], position: [-1, 0.1, 5], color: 0xf0f3f7, intensity: 0.9 },
    { size: [1.2, 4], position: [-3, 1, -4], color: 0xffffff, intensity: 2.0 },
  ];
  for (const { size, position, color, intensity } of cards) {
    const material = new MeshBasicMaterial({ color: new Color(color).multiplyScalar(intensity) });
    const card = new Mesh(new PlaneGeometry(...size), material);
    card.position.set(...position);
    card.lookAt(0, 0, 0);
    studio.add(card);
  }
  const generator = new PMREMGenerator(renderer);
  const target = generator.fromScene(studio, 0.02);
  generator.dispose();
  for (const card of studio.children) {
    card.geometry.dispose();
    card.material.dispose();
  }
  return target;
}

export function createDeviceScreenMaterial() {
  const material = new MeshBasicMaterial({ color: 0xffffff, toneMapped: false });
  material.transparent = true;
  return material;
}

export function cloneDeviceSurfaceMaterial(material, maximumAnisotropy) {
  const clone = material.clone();
  if (clone.map) clone.map.anisotropy = Math.min(MAX_ANISOTROPY, maximumAnisotropy);
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

function webglContext(canvas) {
  if (!canvas || typeof canvas.getContext !== "function") {
    throw new Error("The device stage needs a canvas element.");
  }
  const context = canvas.getContext("webgl2", {
    alpha: true,
    antialias: true,
    premultipliedAlpha: true,
    powerPreference: "high-performance",
  });
  if (!context) throw new Error("The device stage needs WebGL and this canvas has none.");
  return context;
}

function createRenderer(canvas) {
  const renderer = new WebGLRenderer({ canvas, context: webglContext(canvas) });
  renderer.setClearColor(new Color(0x000000), 0);
  renderer.outputColorSpace = SRGBColorSpace;
  renderer.toneMapping = ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.04;
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = PCFShadowMap;
  return renderer;
}

// Keep one vertical world unit at the layout plane, while giving the hardware
// photographic convergence and spatially varying reflections.
function createCamera() {
  const camera = new PerspectiveCamera(STAGE_CAMERA.fovDeg, 1, STAGE_CAMERA.near, STAGE_CAMERA.far);
  camera.position.set(STAGE_CAMERA.position.x, STAGE_CAMERA.position.y, STAGE_CAMERA.position.z);
  camera.lookAt(0, 0, 0);
  return camera;
}

function addStudioLights(scene) {
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
  scene.add(sky, key, key.target, fill, edge);
  return key;
}

// The key light's shadow camera follows the laptop: the keyboard is the only
// place a shadow reads on this stage.
function fitKeyboardShadow(key, laptop) {
  key.castShadow = Boolean(laptop?.root.visible);
  if (!key.castShadow) return;
  const center = laptop.root.position;
  const span = laptop.width * laptop.root.scale.x * 0.72;
  key.target.position.copy(center);
  key.position.copy(center).add({ x: -2, y: 3, z: 4 });
  const shadowCamera = key.shadow.camera;
  Object.assign(shadowCamera, {
    left: -span,
    right: span,
    top: span,
    bottom: -span,
    near: Math.sqrt(29) - span * 2,
    far: Math.sqrt(29) + span * 2,
  });
  shadowCamera.updateProjectionMatrix();
}

function prepareScreenMesh(node, name) {
  node.material = createDeviceScreenMaterial();
  node.userData.deviceScreen = name;
}

function prepareModel(name, source, maximumAnisotropy) {
  source.updateMatrixWorld(true);
  const [anchorX, anchorY, anchorZ] = deviceAnchor(name);
  source.position.set(-anchorX, -anchorY, -anchorZ);
  const contract = DEVICE_CONTRACT.devices[name];
  const screenMeshes = [];
  source.traverse((node) => {
    if (!(node instanceof Mesh)) return;
    node.castShadow = !/screen|legend/.test(node.name);
    node.receiveShadow = node.castShadow;
    if (node.name === contract.screen.node) {
      prepareScreenMesh(node, name);
      screenMeshes.push(node);
      return;
    }
    const materials = Array.isArray(node.material) ? node.material : [node.material];
    const clones = materials.map((material) => cloneDeviceSurfaceMaterial(material, maximumAnisotropy));
    node.material = Array.isArray(node.material) ? clones : clones[0];
  });
  if (!screenMeshes.length) throw new Error(`Missing ${name} screen mesh`);
  const hingeName = contract.lid_hinge_node;
  const lidHinge = hingeName ? source.getObjectByName(hingeName) : null;
  if (hingeName && !lidHinge) throw new Error(`Missing ${name} lid hinge node`);
  const root = new Group();
  root.name = `device-${name}`;
  root.add(source);
  root.visible = false;
  return {
    name,
    root,
    source,
    screenMeshes,
    lidHinge,
    screenUrl: null,
    shown: true,
    opacity: 0,
    pose: normalizePose(),
    width: contract.bounds_size_m[0],
  };
}

function setModelOpacity(model, opacity) {
  model.source.traverse((node) => {
    if (!node.isMesh) return;
    const materials = Array.isArray(node.material) ? node.material : [node.material];
    for (const material of materials) {
      material.opacity = opacity;
      // Depth writes stay on while fading: without them the keyboard and hinge
      // draw through the lid and the laptop reads as a grey silhouette.
      material.depthWrite = true;
    }
  });
}

function applyPose(model, pose, aspect) {
  const world = poseToWorld(model.name, pose, { aspect });
  model.pose = pose;
  model.opacity = world.opacity;
  model.root.visible = model.shown && world.opacity > VISIBLE_OPACITY && pose.w > 0;
  if (!model.root.visible) return;
  model.root.scale.setScalar(world.scale);
  model.root.position.set(world.position.x, world.position.y, world.position.z);
  model.root.rotation.set(world.rotation.pitch, world.rotation.yaw, world.rotation.roll, "YXZ");
  if (model.lidHinge) model.lidHinge.rotation.x = world.lidRotation;
  setModelOpacity(model, world.opacity);
}

function disposeObject(root) {
  root.traverse((node) => {
    if (!node.isMesh) return;
    node.geometry?.dispose();
    const materials = Array.isArray(node.material) ? node.material : [node.material];
    for (const material of materials) material?.dispose?.();
  });
}

export function createDeviceStage({
  canvas,
  assetBase = "/landing/assets/devices",
  screenBase = "/landing/assets/screens",
} = {}) {
  const renderer = createRenderer(canvas);
  const scene = new Scene();
  const camera = createCamera();
  const keyLight = addStudioLights(scene);
  const environment = createDeviceEnvironment(renderer);
  scene.environment = environment.texture;
  scene.environmentIntensity = 1;
  const depthMaterial = new MeshBasicMaterial();
  depthMaterial.colorWrite = false;
  depthMaterial.depthWrite = true;
  depthMaterial.depthTest = true;
  const loader = new GLTFLoader();
  const textureLoader = new TextureLoader();
  const models = new Map();
  const modelPromises = new Map();
  const textures = new Map();
  const texturePromises = new Map();
  const desiredScreens = new Map();
  let loadQueue = Promise.resolve();
  let qualityScale = 1;
  let aspect = 1;
  let viewport = { width: 1, height: 1 };
  let lastFrameMs = 0;
  let disposed = false;

  function resize() {
    const width = canvas.clientWidth || canvas.width || 0;
    const height = canvas.clientHeight || canvas.height || 0;
    if (width < 1 || height < 1) return;
    viewport = { width, height };
    aspect = width / height;
    // 1.5x is enough for a device render behind copy; 2x doubles the fill cost
    // on every scroll frame.
    const pixelRatio = Math.min(globalThis.devicePixelRatio || 1, MAX_PIXEL_RATIO) * qualityScale;
    renderer.setPixelRatio(pixelRatio);
    renderer.setSize(width, height, false);
    camera.aspect = aspect;
    camera.updateProjectionMatrix();
  }

  function letterbox(source, deviceName) {
    const image = source.image;
    const width = image.naturalWidth || image.width;
    const height = image.naturalHeight || image.height;
    const layout = getContainedTextureLayout(width, height, deviceScreenAspect(deviceName));
    if (layout.width === width && layout.height === height) return source;
    const target = canvas.ownerDocument.createElement("canvas");
    target.width = layout.width;
    target.height = layout.height;
    const context = target.getContext("2d");
    if (!context) return source;
    context.fillStyle = LETTERBOX_COLOR;
    context.fillRect(0, 0, target.width, target.height);
    context.drawImage(image, layout.x, layout.y, width, height);
    const texture = new CanvasTexture(target);
    source.dispose();
    return texture;
  }

  function dressTexture(texture) {
    texture.colorSpace = SRGBColorSpace;
    // Preserve small system glyphs and key legends as the display turns.
    texture.anisotropy = Math.min(MAX_ANISOTROPY, renderer.capabilities.getMaxAnisotropy());
    texture.flipY = false;
    texture.needsUpdate = true;
    return texture;
  }

  function loadTexture(deviceName, url) {
    const key = `${deviceName}:${url}`;
    if (textures.has(key)) return Promise.resolve(textures.get(key));
    if (texturePromises.has(key)) return texturePromises.get(key);
    const promise = textureLoader.loadAsync(url).then((source) => {
      if (disposed) {
        source.dispose();
        throw new Error("the device stage is disposed");
      }
      const texture = dressTexture(letterbox(source, deviceName));
      textures.set(key, texture);
      return texture;
    }).finally(() => texturePromises.delete(key));
    texturePromises.set(key, promise);
    return promise;
  }

  function showTexture(model, texture, url) {
    for (const screen of model.screenMeshes) {
      screen.material.map = texture;
      screen.material.color.setHex(0xffffff);
      screen.material.needsUpdate = true;
    }
    model.screenUrl = url;
  }

  async function loadModel(name) {
    const gltf = await loader.loadAsync(modelUrl(assetBase, name));
    if (disposed) {
      disposeObject(gltf.scene);
      return undefined;
    }
    const model = prepareModel(name, gltf.scene, renderer.capabilities.getMaxAnisotropy());
    models.set(name, model);
    modelPromises.delete(name);
    scene.add(model.root);
    const pending = desiredScreens.get(name);
    if (pending) await applyScreen(model, pending);
    return model;
  }

  // Devices load one at a time: a second GLB competing for bandwidth only
  // delays the one the visitor is looking at.
  function ensureModel(name) {
    if (models.has(name)) return Promise.resolve(models.get(name));
    if (modelPromises.has(name)) return modelPromises.get(name);
    const promise = loadQueue.then(() => loadModel(name));
    loadQueue = promise.catch(() => undefined);
    modelPromises.set(name, promise);
    return promise;
  }

  async function applyScreen(model, url) {
    const texture = await loadTexture(model.name, url);
    // The previous display stays until the next one is decoded, and a screen
    // that was superseded while loading never reaches the glass.
    if (disposed || desiredScreens.get(model.name) !== url) return;
    showTexture(model, texture, url);
  }

  function visibleModels() {
    return DEVICE_LAYER_ORDER
      .map((name) => [name, models.get(name)])
      .filter(([, model]) => model && model.root.visible);
  }

  function render() {
    if (disposed || !viewport.width) return 0;
    const startedAt = performance.now();
    for (const model of models.values()) applyPose(model, model.pose, aspect);
    fitKeyboardShadow(keyLight, models.get("laptop"));
    renderSolidDeviceFades(renderer, scene, camera, depthMaterial, visibleModels());
    lastFrameMs = performance.now() - startedAt;
    return lastFrameMs;
  }

  // What a frame costs on this machine. A slow renderer is a reason to stop
  // drawing 3D, never a reason to take the story apart: the caller compares
  // this with the frame budget and falls back to the posters.
  function measureFrameCost({ frames = 5, warmup = 2 } = {}) {
    const samples = [];
    for (let index = 0; index < warmup + frames; index += 1) {
      const cost = render();
      if (index >= warmup) samples.push(cost);
    }
    return frameCostFromSamples(samples);
  }

  function releaseUnusedScreens() {
    const wanted = new Set();
    for (const [name, model] of models) {
      if (model.screenUrl) wanted.add(`${name}:${model.screenUrl}`);
      const desired = desiredScreens.get(name);
      if (desired) wanted.add(`${name}:${desired}`);
    }
    for (const [key, texture] of textures) {
      if (wanted.has(key)) continue;
      texture.dispose();
      textures.delete(key);
    }
  }

  function dispose() {
    if (disposed) return;
    disposed = true;
    for (const model of models.values()) {
      scene.remove(model.root);
      disposeObject(model.root);
    }
    models.clear();
    desiredScreens.clear();
    for (const texture of textures.values()) texture.dispose();
    textures.clear();
    environment.dispose();
    depthMaterial.dispose();
    renderer.dispose();
    renderer.forceContextLoss?.();
  }

  resize();

  return {
    load: (deviceNames) => Promise.all([].concat(deviceNames).map(ensureModel)),
    setScreen(deviceName, textureName) {
      const url = screenUrl(screenBase, textureName);
      desiredScreens.set(deviceName, url);
      const model = models.get(deviceName);
      return model ? applyScreen(model, url) : ensureModel(deviceName).then(() => undefined);
    },
    // Decode a display before the beat that needs it; it stays cached until it
    // is used or releaseUnusedScreens() is called.
    preloadScreen: (deviceName, textureName) => loadTexture(deviceName, screenUrl(screenBase, textureName)),
    releaseUnusedScreens,
    setPose(deviceName, pose) {
      const model = models.get(deviceName);
      const normalized = normalizePose(pose);
      if (model) applyPose(model, normalized, aspect);
      return normalized;
    },
    show(deviceName) {
      const model = models.get(deviceName);
      if (model) model.shown = true;
    },
    hide(deviceName) {
      const model = models.get(deviceName);
      if (!model) return;
      model.shown = false;
      model.root.visible = false;
    },
    // The display's four corners in CSS pixels, for aligning an HTML overlay.
    screenCorners: (deviceName, pose) => screenCornersPx(
      deviceName,
      pose || models.get(deviceName)?.pose || normalizePose(),
      viewport,
    ),
    setQualityScale(scale) {
      qualityScale = scale;
      resize();
    },
    isSoftwareRenderer() {
      const context = renderer.getContext();
      const extension = context.getExtension("WEBGL_debug_renderer_info");
      const name = extension
        ? context.getParameter(extension.UNMASKED_RENDERER_WEBGL)
        : context.getParameter(context.RENDERER);
      return /swiftshader|llvmpipe|software rasterizer/i.test(String(name));
    },
    getState: () => ({
      devices: [...models.keys()],
      screens: Object.fromEntries([...models].map(([name, model]) => [name, model.screenUrl])),
      viewport,
      lastFrameMs,
      qualityScale,
      disposed,
    }),
    resize,
    render,
    measureFrameCost,
    dispose,
  };
}
