/** @vitest-environment jsdom */
// The workspace lifecycle settings (#167), from the wire to the Settings
// sheet with nothing in between stood in: the device page stands up every
// bridge panel, the answer is the contract fixture, and it reaches the panel
// through the real settings cache. Only the machine is a fake: `callRpc`.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import settingsGet from "../../fixtures/api/v1/settings.get.json";
import settingsSet from "../../fixtures/api/v1/settings.set.json";

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = `<div id="projects"></div><div id="bridge"></div>`;
});

describe("the workspace lifecycle settings, wire to sheet", () => {
  it("paints settings.get's answer and saves through settings.set", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "settings.get") return settingsGet.result;
      if (method === "settings.set") return settingsSet.result;
      if (method === "models.list") return { providers: [] };
      return {};
    });
    const { standUpDevicePanels } = await import("../src/views/devicePanels.js");
    standUpDevicePanels({
      projectsHost: document.querySelector("#projects"),
      bridgeHost: document.querySelector("#bridge"),
      callRpc,
      device: { id: "dev-1", name: "laptop" },
    });
    const hours = () => document.querySelector("#workspaceidlehours");
    const prune = () => document.querySelector("#workspaceprune");

    await vi.waitFor(() => expect(hours().value).toBe("24"));
    expect(prune().disabled).toBe(true);
    expect(document.querySelector("#workspaceprunepinned").textContent)
      .toBe("BRIDGE_WORKSPACE_PRUNE sets this on this machine.");

    hours().value = "48";
    await hours().onchange();

    expect(callRpc).toHaveBeenCalledWith("settings.set", { workspace_idle_secs: settingsSet.params.workspace_idle_secs });
    await vi.waitFor(() => expect(hours().value).toBe("48"));
    expect(prune().checked).toBe(true);
  });
});
