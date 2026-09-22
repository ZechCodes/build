import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { Box3, FrontSide, MathUtils, Raycaster, Vector3 } from "three";
import { GLTFLoader } from "three/addons/loaders/GLTFLoader.js";
import { DEVICE_CONTRACT } from "../../skriftapp/buildapp/landing/assets/devices/device-contract.js";

const assetRoot = new URL("../../skriftapp/buildapp/landing/assets/devices/", import.meta.url);
const devices = [
  { name: "laptop", file: "laptop-low.glb", panel: [3024, 1964], body: [0.3126, 0.011, 0.2212] },
  { name: "tablet", file: "tablet.glb", panel: [2420, 1668], body: [0.2497, 0.1775, 0.0053] },
  { name: "phone", file: "phone.glb", panel: [1320, 2868], body: [0.078, 0.1634, 0.00875] },
];

async function loadDevice(file) {
  const buffer = await readFile(new URL(file, assetRoot));
  const gltf = await new GLTFLoader().parseAsync(
    buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength),
    "",
  );
  gltf.scene.updateMatrixWorld(true);
  return gltf.scene;
}

function meshNamed(scene, name) {
  const mesh = scene.getObjectByName(name);
  assert(mesh?.isMesh, `missing exported mesh ${name}`);
  return mesh;
}

function worldVertices(mesh) {
  const position = mesh.geometry.attributes.position;
  const point = new Vector3();
  return Array.from({ length: position.count }, (_, index) => (
    point.fromBufferAttribute(position, index).clone().applyMatrix4(mesh.matrixWorld)
  ));
}

function topIntersection(object, x, z) {
  const hits = new Raycaster(
    new Vector3(x, 0.05, z),
    new Vector3(0, -1, 0),
  ).intersectObject(object, true);
  assert(hits.length > 0, `vertical ray missed ${object.name}`);
  return hits[0];
}

function roundedCornerProfile(mesh) {
  const vertices = worldVertices(mesh);
  const maxX = Math.max(...vertices.map(({ x }) => x));
  const maxY = Math.max(...vertices.map(({ y }) => y));
  const centerX = Math.min(...vertices
    .filter(({ x, y }) => x > 0 && maxY - y < 0.000001)
    .map(({ x }) => x));
  const centerY = Math.min(...vertices
    .filter(({ x, y }) => y > 0 && maxX - x < 0.000001)
    .map(({ y }) => y));
  return {
    centerX,
    centerY,
    radiusX: maxX - centerX,
    radiusY: maxY - centerY,
  };
}

for (const { name, file, panel, body } of devices) {
  test(`${name} exported display matches its 2025 panel and is visible through the hardware`, async () => {
    const contract = DEVICE_CONTRACT.devices[name];
    assert.deepEqual(contract.body_size_m, body);
    assert(Math.abs(contract.screen.size_m[0] / contract.screen.size_m[1] - panel[0] / panel[1]) < 0.00001);
    const scene = await loadDevice(file);
    const screens = [];
    scene.traverse((node) => { if (node.name === "screen" && node.isMesh) screens.push(node); });
    assert.equal(screens.length, 1);
    const corners = contract.screen.corners_m.map((corner) => new Vector3(...corner));
    const horizontal = corners[1].clone().sub(corners[0]);
    const vertical = corners[3].clone().sub(corners[0]);
    const normal = horizontal.clone().cross(vertical).normalize();
    assert(Math.abs(horizontal.length() - contract.screen.size_m[0]) < 0.000001);
    assert(Math.abs(vertical.length() - contract.screen.size_m[1]) < 0.000001);
    for (const x of [0.15, 0.5, 0.85]) {
      for (const y of [0.15, 0.5, 0.85]) {
        const point = corners[0].clone().addScaledVector(horizontal, x).addScaledVector(vertical, y);
        const origin = point.clone().addScaledVector(normal, 0.1);
        const intersections = new Raycaster(origin, normal.clone().negate()).intersectObject(scene, true);
        assert.equal(intersections[0]?.object.name, "screen", `${name} display is covered at ${x},${y}`);
        assert(intersections[0].point.distanceTo(point) < 0.000001, "exported screen agrees with the runtime contract");
      }
    }
  });
}

test("exported enclosure sides face out toward the viewer", async () => {
  const shells = [
    { file: "laptop-low.glb", mesh: "laptop_hardware", axes: ["x", "z"] },
    { file: "tablet.glb", mesh: "tablet_body", axes: ["x", "y"] },
    { file: "phone.glb", mesh: "phone_body", axes: ["x", "y"] },
  ];

  for (const spec of shells) {
    const scene = await loadDevice(spec.file);
    const shell = meshNamed(scene, spec.mesh);
    const bounds = new Box3().setFromObject(shell);
    const center = bounds.getCenter(new Vector3());
    const materials = Array.isArray(shell.material) ? shell.material : [shell.material];
    const originalSides = materials.map(({ side }) => side);
    materials.forEach((material) => { material.side = FrontSide; });

    try {
      for (const axis of spec.axes) {
        for (const sign of [-1, 1]) {
          const origin = center.clone();
          const direction = new Vector3();
          const nearEdge = sign < 0 ? bounds.min[axis] : bounds.max[axis];
          const farEdge = sign < 0 ? bounds.max[axis] : bounds.min[axis];
          origin[axis] = nearEdge + sign * 0.01;
          direction[axis] = -sign;
          const hit = new Raycaster(origin, direction, 0, 0.5).intersectObject(shell, false)[0];
          assert(hit, `${spec.mesh} has no front-facing ${axis}${sign < 0 ? "-" : "+"} side`);
          assert(
            Math.abs(hit.point[axis] - nearEdge) < Math.abs(hit.point[axis] - farEdge),
            `${spec.mesh} ${axis}${sign < 0 ? "-" : "+"} side is wound inward`,
          );
        }
      }
    } finally {
      materials.forEach((material, index) => { material.side = originalSides[index]; });
    }
  }
});

test("laptop enclosure has a solid deck over a rolled underbody section", async () => {
  const scene = await loadDevice("laptop-low.glb");
  const hardware = meshNamed(scene, "laptop_hardware");
  const levels = hardware.userData.profile_levels_m;
  const insets = hardware.userData.profile_insets_m;
  assert.equal(hardware.userData.profile_kind, "rolled_underbody");
  assert.equal(levels.length, 16);
  assert.equal(insets.length, levels.length);
  for (let step = 0; step <= 8; step += 1) {
    const angle = Math.PI * step / 16;
    assert(Math.abs(levels[step] - 0.004 * (1 - Math.cos(angle))) < 0.0000001);
    assert(Math.abs(insets[step] - 0.0018 * (1 - Math.sin(angle))) < 0.0000001);
  }
  assert.equal(levels[9], 0.0082);
  assert.equal(insets[9], 0);
  for (let step = 1; step <= 6; step += 1) {
    const index = 9 + step;
    const angle = Math.PI * step / 12;
    assert(Math.abs(levels[index] - (0.0082 + 0.0028 * Math.sin(angle))) < 0.0000001);
    assert(Math.abs(insets[index] - 0.0008 * (1 - Math.cos(angle))) < 0.0000001);
  }

  const vertices = worldVertices(hardware);
  const halfWidth = DEVICE_CONTRACT.devices.laptop.body_size_m[0] / 2;
  const bodyDepth = DEVICE_CONTRACT.devices.laptop.body_size_m[2];
  const sections = levels.map((height, index) => {
    const ring = vertices.filter(({ y, z }) => Math.abs(y - height) < 0.0000001 && z > bodyDepth * 0.8);
    assert(ring.length > 20, `missing laptop shell ring at ${height}m`);
    return {
      maxX: Math.max(...ring.map(({ x }) => x)),
      front: Math.max(...ring.map(({ z }) => z)),
      inset: insets[index],
    };
  });
  for (const section of sections) {
    assert(Math.abs(section.maxX - (halfWidth - section.inset)) < 0.000001);
    assert(Math.abs(section.front - (bodyDepth - section.inset)) < 0.000001);
  }
  assert(sections.slice(1, 9).every((section, index) => section.front > sections[index].front));
  assert(Math.abs(sections[8].front - sections[9].front) < 0.000001, "port belt is not straight");
  assert(sections.slice(10).every((section, index) => section.front < sections[index + 9].front));

  const ports = new Box3().setFromObject(scene.getObjectByName("laptop_ports"));
  assert(ports.min.x < -halfWidth && ports.max.x > halfWidth, "port apertures are buried inside the side belt");
  assert(ports.min.y < levels[9] && ports.max.y > levels[8], "ports are not seated across the straight belt");

  for (const [x, z] of [
    [0, 0.205], [-0.14, 0.18], [0.14, 0.18],
    [-0.145, 0.11], [0.145, 0.11], [0, 0.005],
  ]) {
    const deck = topIntersection(hardware, x, z);
    assert.equal(deck.object.name, "laptop_hardware");
    assert(Math.abs(deck.point.y - 0.011) < 0.000001, `deck is open at ${x},${z}`);
  }
});

test("laptop export has raised dished keys above a recessed keyboard well", async () => {
  const scene = await loadDevice("laptop-low.glb");
  const hardware = meshNamed(scene, "laptop_hardware");
  const well = meshNamed(scene, "laptop_insets");
  const keyGroup = scene.getObjectByName("keyboard_keys");
  assert(keyGroup, "missing exported keyboard_keys group");
  const keys = keyGroup.children.find((child) => child.isMesh && child.material?.name === "KeyGraphite");
  assert(keys, "missing exported keycap geometry");

  const wellBounds = new Box3().setFromObject(well);
  const wellCenter = wellBounds.getCenter(new Vector3());
  const wellTop = topIntersection(well, wellCenter.x, wellCenter.z).point.y;
  const pocketTop = topIntersection(hardware, wellCenter.x, wellCenter.z).point.y;
  const deckTop = topIntersection(hardware, wellBounds.max.x + 0.01, wellCenter.z).point.y;
  assert(deckTop - pocketTop > 0.0004, "keyboard pocket is not cut below the surrounding deck");
  assert(pocketTop < wellTop && wellTop < deckTop, "keyboard well floor does not sit inside the deck recess");

  const keyBounds = new Box3().setFromObject(keys);
  const firstKeyVertices = worldVertices(keys).filter(({ x, z }) => (
    x < keyBounds.min.x + 0.017 && z < keyBounds.min.z + 0.0135
  ));
  assert(firstKeyVertices.length > 20, "could not sample an exported keycap");
  const firstKeyBounds = new Box3().setFromPoints(firstKeyVertices);
  const firstKeyCenter = firstKeyBounds.getCenter(new Vector3());
  const keyTop = topIntersection(keys, firstKeyCenter.x, firstKeyCenter.z).point.y;
  assert(keyTop - wellTop > 0.0008, "keycaps are not physically raised above the keyboard well");

  const upperProfile = firstKeyVertices.filter(({ y }) => y > wellTop + 0.0008);
  const shoulderHeight = Math.max(...upperProfile.map(({ y }) => y));
  const innerTop = upperProfile.reduce((closest, vertex) => {
    const distance = Math.hypot(vertex.x - firstKeyCenter.x, vertex.z - firstKeyCenter.z);
    return distance < closest.distance ? { distance, height: vertex.y } : closest;
  }, { distance: Infinity, height: Infinity });
  const dishDepth = shoulderHeight - innerTop.height;
  assert(dishDepth > 0.00007 && dishDepth < 0.00015, `keycap dish depth is ${dishDepth}m`);
});

test("tablet screen corners stay concentric with the enclosure", async () => {
  const scene = await loadDevice("tablet.glb");
  const body = roundedCornerProfile(meshNamed(scene, "tablet_body"));
  const screen = roundedCornerProfile(meshNamed(scene, "screen"));
  const centerOffset = Math.hypot(body.centerX - screen.centerX, body.centerY - screen.centerY);

  assert(body.radiusX > 0.0145 && body.radiusX < 0.0155, `tablet body radius is ${body.radiusX}m`);
  assert(screen.radiusX > 0.0063 && screen.radiusX < 0.0069, `tablet screen radius is ${screen.radiusX}m`);
  assert(Math.abs(body.radiusX - body.radiusY) < 0.00001, "tablet body corner is not circular");
  assert(Math.abs(screen.radiusX - screen.radiusY) < 0.00001, "tablet screen corner is not circular");
  assert(centerOffset < 0.000075, `tablet nested corner centers differ by ${centerOffset}m`);
});

test("exported screens sit safely in front of their cover glass", async () => {
  const glassNames = {
    laptop: "laptop_front_glass",
    tablet: "tablet_front_glass",
    phone: "phone_front_glass",
  };

  for (const { name, file } of devices) {
    const scene = await loadDevice(file);
    const contract = DEVICE_CONTRACT.devices[name];
    const corners = contract.screen.corners_m.map((corner) => new Vector3(...corner));
    const normal = corners[1].clone().sub(corners[0])
      .cross(corners[3].clone().sub(corners[0]))
      .normalize();
    const screenDepth = Math.min(...worldVertices(meshNamed(scene, "screen")).map((point) => point.dot(normal)));
    const glassDepth = Math.max(...worldVertices(meshNamed(scene, glassNames[name])).map((point) => point.dot(normal)));
    const clearance = screenDepth - glassDepth;
    assert(clearance > 0.000075, `${name} screen clears its glass by only ${clearance}m`);
    assert(Math.abs(clearance - contract.screen.surface_clearance_m) < 0.000001);
  }
});

test("laptop lid exports a physical hinge with stable open and closed endpoints", async () => {
  const contract = DEVICE_CONTRACT.devices.laptop;
  const scene = await loadDevice("laptop-low.glb");
  const hinge = scene.getObjectByName(contract.lid_hinge_node);
  const screen = scene.getObjectByName(contract.screen.node);
  assert(hinge, "exported laptop is missing its hinge node");
  assert.equal(screen?.parent, hinge, "display must move with the lid hinge");
  assert(hinge.position.distanceTo(new Vector3(...contract.hinge_pivot_m)) < 0.000001);
  assert.equal(hinge.userData.hinge_axis, contract.lid_hinge_axis);
  assert.equal(hinge.userData.closed_rotation_deg, contract.lid_hinge_closed_rotation_deg);
  assert(Math.abs(MathUtils.radToDeg(hinge.rotation.x) - contract.lid_hinge_default_rotation_deg) < 0.00001);
  assert.deepEqual(
    new Set(hinge.children.map(({ name }) => name)),
    new Set(["camera_notch", "camera_notch_bridge", "facetime_camera", "laptop_front_glass", "laptop_lid_shell", "screen"]),
  );

  const hardwareBounds = new Box3().setFromObject(meshNamed(scene, "laptop_hardware"));
  const keyBounds = new Box3().setFromObject(scene.getObjectByName("keyboard_keys"));
  const openScreenNormal = new Vector3(0, 0, 1).applyQuaternion(hinge.quaternion);
  assert(openScreenNormal.y > 0.2 && openScreenNormal.z > 0.9, "open display does not face the viewer");

  hinge.rotation.x = MathUtils.degToRad(contract.lid_hinge_closed_rotation_deg);
  scene.updateMatrixWorld(true);
  const closedBounds = new Box3().setFromObject(screen);
  const closedLidBounds = new Box3().setFromObject(hinge);
  const closedScreenNormal = new Vector3(0, 0, 1).applyQuaternion(hinge.quaternion);
  assert(closedScreenNormal.y < -0.99 && Math.abs(closedScreenNormal.z) < 0.0001, "closed display does not face the keyboard");
  assert(closedBounds.min.x > hardwareBounds.min.x && closedBounds.max.x < hardwareBounds.max.x);
  assert(closedBounds.min.z > hardwareBounds.min.z && closedBounds.max.z < hardwareBounds.max.z);
  assert(closedBounds.min.y > contract.body_size_m[1], "closed display intersects the keyboard deck");
  assert(closedBounds.min.y > keyBounds.max.y, "closed display intersects the keycaps");
  assert(
    Math.abs(closedLidBounds.max.y - contract.closed_height_m) < 0.00025,
    `closed laptop height is ${closedLidBounds.max.y}m`,
  );
});
