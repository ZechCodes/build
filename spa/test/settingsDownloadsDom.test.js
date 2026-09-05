// @vitest-environment jsdom
// Settings → Downloads: the same block the first-run screen shows, for the
// second machine and for updating this one. It sits above Devices & keys,
// which is where the empty-device sentence now sends people.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { asset, downloadsPayload } from "./downloadsFixture.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const DOWNLOADS = downloadsPayload({
  platforms: [{ key: "linux-x86_64", label: "Linux · x86_64", url: asset("linux-x86_64") }],
});

const CATALOG = {
  default_provider: "claude",
  providers: [{ id: "claude", label: "Claude Code", models: [], efforts: [], creatable: true }],
};

let devices = [];
let downloads = async () => DOWNLOADS;
const fetchDownloads = vi.fn(() => downloads());

const call = vi.fn(async (method) => {
  if (method === "project.list") return { projects: [] };
  if (method === "settings.get") return { projects_dir: "~/code", default_harness: "claude" };
  if (method === "models.list") return CATALOG;
  return {};
});

vi.mock("../src/app.js", () => ({
  App: { call: (...args) => call(...args) },
  loadModelCatalog: async () => CATALOG,
}));
vi.mock("../src/api.js", () => ({
  revokeDevice: async () => {},
  fetchDownloads: (...args) => fetchDownloads(...args),
}));
vi.mock("../src/devices.js", () => ({ refreshDevices: async () => devices }));
vi.mock("../src/push.js", () => ({
  pushState: async () => "unsupported",
  enablePush: async () => {},
  disablePush: async () => {},
}));
vi.mock("../src/core/platform.js", () => ({ currentPlatformKey: () => "linux-x86_64" }));
vi.mock("../src/sheets/browser.js", () => ({ openBrowser: () => {} }));
vi.mock("../src/sheets/newRepo.js", () => ({ openNewRepo: () => {} }));
vi.mock("../src/sheets/setRemote.js", () => ({ openSetRemote: () => {} }));
vi.mock("../src/sheets/clone.js", () => ({ openClone: () => {} }));
vi.mock("../src/sheets/addDevice.js", () => ({ openAddDevice: () => {} }));

const flush = () => new Promise((done) => setTimeout(done, 0));

let renderSettings;

beforeEach(async () => {
  vi.clearAllMocks();
  devices = [];
  downloads = async () => DOWNLOADS;
  document.body.innerHTML = bodyHtml;
  ({ renderSettings } = await import("../src/views/settings.js"));
});

describe("Settings → Downloads", () => {
  it("mounts the same downloads block the gate shows, for this browser's platform", async () => {
    await renderSettings();
    await flush();
    expect(document.querySelector("#downloads a.btn.primary").getAttribute("href")).toBe(DOWNLOADS.platforms[0].url);
    expect(document.getElementById("installcmd").textContent).toBe(DOWNLOADS.install_command);
  });

  it("sends an account with no devices to that panel rather than to a command", async () => {
    await renderSettings();
    await flush();
    expect(document.getElementById("devlist").textContent).toContain(
      "Install the bridge above, then add it with its pairing code",
    );
    expect(document.getElementById("adddev")).toBeTruthy();
  });

  it("wires the rest of Settings without waiting on the downloads round trip", async () => {
    downloads = () => new Promise(() => {});
    renderSettings();
    await flush();
    expect(typeof document.getElementById("adddev").onclick).toBe("function");
    expect(typeof document.getElementById("newrepo").onclick).toBe("function");
    expect(document.getElementById("downloads").textContent).toContain("loading…");
  });

  it("keeps the devices panel intact when the downloads route refuses", async () => {
    downloads = async () => {
      throw new Error("invite only");
    };
    devices = [{ id: "d1", name: "studio", fingerprint: "AAAABBBBCCCCDDDDEEEE", status: "online" }];
    await renderSettings();
    await flush();
    expect(document.getElementById("downloadserr").textContent).toBe("invite only");
    expect(document.getElementById("devlist").textContent).toContain("studio");
    expect(document.querySelector("#devlist .revoke")).toBeTruthy();
  });
});
