// @vitest-environment jsdom
// The first-run screen: an account with no approved device. It is the same
// three-step gate it has always been — the pairing code is still what pairs a
// device — with the download the human needs before step 2 can happen at all.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { asset, downloadsPayload, mintedCommand } from "./downloadsFixture.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const DOWNLOADS = downloadsPayload({
  platforms: [{ key: "macos-arm64", label: "macOS · Apple silicon", url: asset("macos-arm64") }],
});

let devices = [];
let downloads = async () => DOWNLOADS;
const refreshDevices = vi.fn(async () => devices);
const lookupDevice = vi.fn(async () => ({ name: "studio", fingerprint: "AAAA BBBB CCCC DDDD" }));
const approveDevice = vi.fn(async () => {});
const fetchDownloads = vi.fn(() => downloads());
const mintInstallCommand = vi.fn(async () => ({ install_command: mintedCommand(), expires_in_s: 600 }));

vi.mock("../src/api.js", () => ({
  lookupDevice: (...args) => lookupDevice(...args),
  approveDevice: (...args) => approveDevice(...args),
  fetchDownloads: (...args) => fetchDownloads(...args),
  mintInstallCommand: (...args) => mintInstallCommand(...args),
}));
vi.mock("../src/devices.js", () => ({
  refreshDevices: (...args) => refreshDevices(...args),
  paintDevicePicker: () => {},
}));
// The gate opens every online device at once and carries on the moment the
// first one answers. It names no home: which device that is, the account list
// and the user's pick already say, and connection.js takes each device in hand
// as it lands.
//
// One owner of one fact: connection.js greets every device as it lands it, so
// the gate greeting a bridge itself would put a second session.hello on the
// wire at every boot. This mock names only what gate.js imports, and the
// hand-over test below is what fails if a greeting — or a claim on home —
// creeps back in.
const answering = { deviceId: "d1", session: {}, call: async () => ({}) };
vi.mock("../src/connection.js", () => ({
  openDeviceSessions: () => ({ first: Promise.resolve(answering), settled: Promise.resolve([answering]) }),
  openDeviceSettingsSession: async () => answering,
  syncHome: () => {},
  forgetHomeFollow: () => {},
  setConn: () => {},
  CONNECTION_STATUS: { connected: "", connecting: "", deviceOffline: "", noDevices: "" },
}));
// jsdom is neither a Mac nor a Linux desktop; the platform table has its own
// test, and this one is about what the gate does with the key it is handed.
vi.mock("../src/core/platform.js", () => ({ currentPlatformKey: () => "macos-arm64" }));
vi.mock("../src/app.js", () => ({ App: { devices: [], selectedDeviceId: null }, render: () => {} }));
vi.mock("../src/core/taskFeed.js", () => ({ startFeed: () => {}, stopFeed: () => {} }));
vi.mock("../src/core/cacheSync.js", () => ({ startCacheSync: () => {} }));
vi.mock("../src/core/inboxShell.js", () => ({ initInboxRail: () => {} }));
vi.mock("../src/core/toolbar.js", () => ({ initToolbar: () => {} }));
vi.mock("../src/sheets/addDevice.js", () => ({ openAddDevice: () => {} }));

const flush = () => new Promise((done) => setTimeout(done, 0));

let boot;

beforeEach(async () => {
  vi.clearAllMocks();
  devices = [];
  downloads = async () => DOWNLOADS;
  document.body.innerHTML = bodyHtml;
  ({ boot } = await import("../src/views/gate.js"));
});

describe("the first-run screen", () => {
  it("offers the download and the pairing code on the one screen", async () => {
    await boot();
    await flush();
    expect(document.querySelector("#downloads a.btn.primary").getAttribute("href")).toBe(DOWNLOADS.platforms[0].url);
    expect(document.getElementById("installcmd").textContent).toBe(DOWNLOADS.install_command);
    expect(document.getElementById("ocode")).toBeTruthy();
    expect(document.getElementById("olookup")).toBeTruthy();
  });

  it("states the YOLO reality plainly instead of implying a sandbox", async () => {
    await boot();
    await flush();
    expect(document.getElementById("root").textContent).toContain(
      "Agents run on your machine in YOLO mode",
    );
  });

  it("no longer asks anyone to paste a development environment line", async () => {
    await boot();
    await flush();
    expect(document.getElementById("root").innerHTML).not.toContain("BRIDGE_API_URL=");
  });

  it("pairs on the code the human typed, uppercased, then boots", async () => {
    await boot();
    await flush();
    document.getElementById("ocode").value = "g6zp-kd2u";
    document.getElementById("olookup").click();
    await flush();
    expect(lookupDevice).toHaveBeenCalledWith("G6ZP-KD2U");
    expect(document.getElementById("opairbox").textContent).toContain("AAAA BBBB CCCC DDDD");

    devices = [{ id: "d1", name: "studio", fingerprint: "AAAA", status: "online" }];
    document.getElementById("oapprove").click();
    await flush();
    expect(approveDevice).toHaveBeenCalledWith("G6ZP-KD2U");
    expect(refreshDevices).toHaveBeenCalledTimes(2);
  });

  // boot() swallows what enterApp throws (a stale status is the ordinary
  // reason), so the hand-over itself is the assertion: the gate reaching for
  // anything connection.js does not hand it would leave the screen gated.
  it("the gate enters the app on the first device that answers and names no home itself", async () => {
    devices = [{ id: "d1", name: "studio", fingerprint: "AAAA", status: "online" }];

    await boot();
    await flush();

    expect(document.body.classList.contains("gated")).toBe(false);
  });

  it("names a lookup refusal without losing the screen", async () => {
    lookupDevice.mockRejectedValueOnce(new Error("no pending device for that code"));
    await boot();
    await flush();
    document.getElementById("ocode").value = "ZZZZ-ZZZZ";
    document.getElementById("olookup").click();
    await flush();
    expect(document.getElementById("oerr").textContent).toBe("no pending device for that code");
    expect(document.getElementById("ocode")).toBeTruthy();
  });

  it("keeps pairing usable when the downloads route refuses", async () => {
    downloads = async () => {
      throw new Error("invite only");
    };
    await boot();
    await flush();
    expect(document.getElementById("downloadserr").textContent).toBe("invite only");
    expect(document.getElementById("installcmd")).toBe(null);
    document.getElementById("ocode").value = "g6zp-kd2u";
    document.getElementById("olookup").click();
    await flush();
    expect(lookupDevice).toHaveBeenCalledWith("G6ZP-KD2U");
  });
});
