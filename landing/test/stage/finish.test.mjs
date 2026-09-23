import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeshPhysicalMaterial, Color } from "three";
import { DEVICE_FINISH } from "../../src/stage/finish.js";
import { cloneDeviceSurfaceMaterial } from "../../src/stage/stage.js";

function luminance(color) {
  return 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
}

describe("the device finish", () => {
  it("dresses every body and edge in silver aluminium, on all three devices", () => {
    for (const name of ["SpaceBlackAluminum", "MachinedSpaceBlackEdge", "DeepBlueAluminum", "DeepBlueMachinedEdge", "TrackpadSpaceBlack"]) {
      const source = new MeshPhysicalMaterial({ name, color: new Color(0.045, 0.05, 0.059), metalness: 1, roughness: 0.3 });
      const dressed = cloneDeviceSurfaceMaterial(source, 4);
      assert.ok(luminance(dressed.color) > 0.5, `${name} reads as silver`);
      assert.ok(dressed.metalness >= 0.9, `${name} is metal`);
      assert.ok(Math.abs(dressed.color.r - dressed.color.b) < 0.05, `${name} is neutral`);
      assert.equal(source.color.r, 0.045, "the source material is left alone");
    }
  });

  it("keeps the keys, glass and screen dark, so the UI stays black and mint", () => {
    for (const name of ["KeyGraphite", "FrontGlass", "BlackInset", "SensorBlack", "ScreenDesktop"]) {
      assert.equal(DEVICE_FINISH[name], undefined, name);
    }
  });
});
