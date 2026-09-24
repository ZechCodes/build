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
