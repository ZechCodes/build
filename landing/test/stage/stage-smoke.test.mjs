// The stage itself needs a GPU, so this only proves the module loads against
// the installed three build and refuses a canvas it cannot draw on.
import assert from "node:assert/strict";
import test from "node:test";

const stage = await import("../../src/stage/stage.js");

function fakeCanvas(context) {
  return {
    clientWidth: 1440,
    clientHeight: 900,
    width: 1440,
    height: 900,
    style: {},
    getContext: () => context,
    addEventListener() {},
    removeEventListener() {},
  };
}

test("the stage module exposes the choreography's api", () => {
  assert.equal(typeof stage.createDeviceStage, "function");
  assert.equal(typeof stage.createDeviceScreenMaterial, "function");
});

test("active screen pixels use an unlit, untone-mapped material", () => {
  const material = stage.createDeviceScreenMaterial();
  assert.equal(material.isMeshBasicMaterial, true);
  assert.equal(material.toneMapped, false);
  assert.equal(material.transparent, true);
  assert.equal(material.color.getHex(), 0xffffff);
});

test("a canvas without WebGL is refused, not half started", () => {
  assert.throws(() => stage.createDeviceStage({}), /canvas/i);
  assert.throws(() => stage.createDeviceStage({ canvas: fakeCanvas(null) }), /WebGL/i);
});

test("the chassis is anodized aluminium: fully metallic, satin, no lacquer", async () => {
  const { MeshPhysicalMaterial } = await import("three");
  for (const name of ["SpaceBlackAluminum", "DeepBlueAluminum"]) {
    const source = new MeshPhysicalMaterial({ name, metalness: 0.4, roughness: 0.34, clearcoat: 0.5 });
    const chassis = stage.cloneDeviceSurfaceMaterial(source, 8);
    assert.equal(chassis.metalness, 1);
    // Satin: rough enough not to read as polished silver, smooth enough to
    // carry the studio's strips as gradients instead of one flat grey.
    assert.ok(chassis.roughness >= 0.4 && chassis.roughness <= 0.5, `${name} roughness ${chassis.roughness}`);
    assert.equal(chassis.clearcoat, 0);
    assert.equal(source.metalness, 0.4, "the loaded material is left alone");
  }
});

test("the trackpad is satin metal, not a lacquered pad", async () => {
  const { MeshPhysicalMaterial } = await import("three");
  const source = new MeshPhysicalMaterial({ name: "TrackpadSpaceBlack", metalness: 0.9, roughness: 0.26, clearcoat: 0.62 });
  const pad = stage.cloneDeviceSurfaceMaterial(source, 8);
  assert.equal(pad.metalness, 1);
  assert.equal(pad.clearcoat, 0);
});

test("the lid rim is darker than the chassis, so it frames the screen quietly", async () => {
  const { MeshPhysicalMaterial } = await import("three");
  const chassis = stage.cloneDeviceSurfaceMaterial(new MeshPhysicalMaterial({ name: "SpaceBlackAluminum" }), 8);
  const rim = chassis.clone();
  stage.dressLidShell(rim);
  assert.equal(rim.metalness, 1);
  assert.ok(rim.color.r <= chassis.color.r * 0.25, `rim ${rim.color.r} vs chassis ${chassis.color.r}`);
});

test("phone and tablet bodies split off only the screen-facing band for the dark rim", async () => {
  const { BoxGeometry } = await import("three");
  const box = new BoxGeometry(1, 1, 1);
  const split = stage.splitFrontBand(box);
  assert.equal(split.groups.length, 2);
  const [sides, band] = split.groups;
  assert.equal(band.materialIndex, 1);
  assert.equal(band.count, 6, "only the +z face's two triangles");
  assert.equal(sides.count, 30);
  assert.equal(box.groups.length, 6, "source geometry untouched");
});
