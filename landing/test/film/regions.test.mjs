// The close-ups redraw regions of the app screens the capture fixture draws
// (design/landing-captures), so each one has to sit inside the app's own
// window on its device's texture, never over the system chrome around it.
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { TEXTURE_SIZES } from "../../src/film/acts.js";

const film = readFileSync(new URL("../../src/components/Film.astro", import.meta.url), "utf8");
const manifest = JSON.parse(readFileSync(new URL("../../../design/landing-captures/screen-manifest.json", import.meta.url), "utf8"));
const PROFILES = { laptop: "macbook", tablet: "ipad", phone: "iphone" };

const panels = [...film.matchAll(/data-panel="([^"]+)" data-panel-device="([^"]+)" data-panel-region="([^"]+)"/g)]
  .map(([, name, device, region]) => ({ name, device, region: region.split(" ").map(Number) }));

describe("the close-ups", () => {
  it("are all read", () => {
    assert.deepEqual(panels.map((panel) => panel.name), ["hole", "task", "team", "builder", "git", "review"]);
  });

  it("are authored in the textures the fixture writes", () => {
    for (const [device, profile] of Object.entries(PROFILES)) {
      assert.deepEqual(TEXTURE_SIZES[device], manifest.profiles.system[profile].derivative, device);
    }
  });

  it("each sit inside the app's window on their device", () => {
    for (const { name, device, region: [x, y, width, height] } of panels) {
      const [left, top, appWidth, appHeight] = manifest.profiles.system[PROFILES[device]].appBounds;
      assert.ok(x >= left && y >= top, `${name} starts inside the app`);
      assert.ok(x + width <= left + appWidth && y + height <= top + appHeight, `${name} ends inside the app`);
    }
  });
});
