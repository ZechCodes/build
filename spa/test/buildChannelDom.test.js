// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const html = readFileSync(resolve("index.html"), "utf8");
const body = html.match(/<body>([\s\S]*)<\/body>/)[1];
const originalTitle = "Build — review is the product";

beforeEach(() => {
  vi.resetModules();
  document.body.innerHTML = body;
  document.body.className = "";
  document.title = originalTitle;
});
afterEach(() => vi.unstubAllEnvs());

async function initialize(channel) {
  vi.stubEnv("VITE_BUILD_CHANNEL", channel);
  const { initBuildChannel } = await import("../src/core/buildChannel.js");
  initBuildChannel();
}

describe("build channel chrome", () => {
  it("exports the configured channel and shows Dev beside the global controls", async () => {
    await initialize("dev");
    expect((await import("../src/config.js")).BUILD_CHANNEL).toBe("dev");
    const badge = document.querySelector("#build-channel");
    expect(badge.textContent).toBe("Dev");
    expect(badge.hidden).toBe(false);
    expect(badge.previousElementSibling.id).toBe("global-controls");
    expect(document.title).toBe("Build Dev — review is the product");
  });

  it.each([undefined, "", "prod", "preview"])("keeps the original UI/title for %s", async (channel) => {
    await initialize(channel);
    expect(document.querySelector("#build-channel").hidden).toBe(true);
    expect(document.title).toBe(originalTitle);
  });

  it("keeps the same badge across routes, welcome and disconnected/empty devices", async () => {
    await initialize("dev");
    const badge = document.querySelector("#build-channel");
    const { App } = await import("../src/appState.js");
    for (const route of ["inbox", "project", "account"]) {
      App.route = { name: route };
      App.devices = [];
      App.selectedDeviceId = null;
      App.gated = true;
      document.body.classList.add("gated");
      document.querySelector("#root").innerHTML = "<h1>Welcome to Build</h1>";
      window.dispatchEvent(new Event("hashchange"));
      expect(document.querySelector("#build-channel")).toBe(badge);
      expect(badge.hidden).toBe(false);
      expect(document.title).toBe("Build Dev — review is the product");
    }
  });
});
