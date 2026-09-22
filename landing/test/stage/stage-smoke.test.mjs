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
