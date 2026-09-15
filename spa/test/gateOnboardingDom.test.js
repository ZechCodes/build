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
let openSession = async () => ({});
let refresh = async () => devices;
const refreshDevices = vi.fn(() => refresh());
// What connection.js hands the gate: every online device opened at once, and
// the promise of the first one to answer. The fixture's `openSession` is the
// one that answers.
let securityStop = "";
const landed = [];
const openDeviceSessions = vi.fn(() => {
  // connection.js registers a machine with the device registry as it lands it;
  // it is mocked out here, so answering is what puts a context on the registry.
  const first = openSession({ preferDeviceId: null }).then((context) => {
    landed.push(context);
    return context;
  });
  return { first, settled: first.then((context) => [context], () => []) };
});
const render = vi.fn();
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
vi.mock("../src/connection.js", () => ({
  chooseCreationDevice: () => {},
  retireDevice: () => {},
  openDeviceSessions: (...args) => openDeviceSessions(...args),
  securityStopText: () => securityStop,
  openDeviceSettingsSession: async () => ({}),
  syncHome: () => {},
  goOffline: () => {},
  forgetHomeFollow: () => {},
  forgetSecurityStops: () => {},
  forgetRendezvousSockets: () => {},
}));
// jsdom is neither a Mac nor a Linux desktop; the platform table has its own
// test, and this one is about what the gate does with the key it is handed.
// The registry as the gate reads it: every machine the fixture has answered
// with. The gate reads it as it starts listening — a greeting can settle while
// the app is still coming up — so a boot that answered has to be a boot the
// registry knows about.
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: (deviceId) => landed.find((context) => context.deviceId === deviceId) || null,
  knownContexts: () => [...landed],
  liveContexts: () => [...landed],
  onDeviceStateChanged: () => () => {},
}));
vi.mock("../src/core/platform.js", () => ({ currentPlatformKey: () => "macos-arm64" }));
vi.mock("../src/app.js", () => ({
  App: { devices: [], selectedDeviceId: null },
  render: (...args) => render(...args),
  unmountView: () => {},
}));
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
  landed.length = 0;
  securityStop = "";
  downloads = async () => DOWNLOADS;
  openSession = async () => ({});
  refresh = async () => devices;
  document.body.innerHTML = bodyHtml;
  const { App } = await import("../src/app.js");
  Object.assign(App, { devices: [], selectedDeviceId: null, _connecting: false, _watch: null });
  ({ boot } = await import("../src/views/gate.js"));
});

describe("the device connection gate", () => {
  it("opens every online device at once and enters on the first that answers", async () => {
    devices = [
      { id: "sticky", name: "Studio", fingerprint: "AAAA", status: "online" },
      { id: "available", name: "Laptop", fingerprint: "BBBB", status: "online" },
    ];
    openSession = async () => ({ deviceId: "available" });
    const { App } = await import("../src/app.js");
    App.devices = devices;
    App.selectedDeviceId = "sticky";

    await boot();

    expect(openDeviceSessions).toHaveBeenCalledTimes(1);
    expect(document.body.classList.contains("gated")).toBe(false);
    expect(document.getElementById("root").textContent).not.toContain("Waiting for your device");
  });

  it("does not claim none are online after an online device's handshake fails", async () => {
    devices = [{ id: "dev-a", name: "Studio", fingerprint: "AAAA", status: "online" }];
    openSession = async () => {
      throw new Error("device did not accept the session");
    };
    const { App } = await import("../src/app.js");
    App.devices = devices;

    await boot();

    expect(document.getElementById("waitintro").textContent).toContain("report online");
    expect(document.getElementById("waitintro").textContent).not.toContain("None of your devices");
    clearInterval(App._watch);
    App._watch = null;
  });

  // A machine whose key did not match the one this account pinned is the one
  // connection failure nobody can wait out: the client has stopped dialling it,
  // so the screen holding the app has to say why rather than watch forever.
  it("says on the waiting screen that a device's key did not match the pinned one", async () => {
    devices = [{ id: "dev-a", name: "Studio", fingerprint: "AAAA", status: "online" }];
    securityStop = "relay-supplied device key does not match the api-pinned key — possible tampering";
    openSession = async () => {
      throw Object.assign(new Error(securityStop), { securityCritical: true });
    };
    const { App } = await import("../src/app.js");
    App.devices = devices;

    await boot();

    expect(document.getElementById("oerr").textContent).toBe(securityStop);
    clearInterval(App._watch);
    App._watch = null;
  });

  it("does not let an older failed boot re-gate a session established by a newer boot", async () => {
    devices = [{ id: "dev-a", name: "Studio", fingerprint: "AAAA", status: "online" }];
    let finishStaleRefresh;
    const staleRefresh = new Promise((resolve) => {
      finishStaleRefresh = resolve;
    });
    let refreshCount = 0;
    refresh = async () => {
      refreshCount += 1;
      return refreshCount === 2 ? staleRefresh : devices;
    };
    openSession = vi
      .fn()
      .mockRejectedValueOnce(new Error("relay warming up"))
      .mockResolvedValueOnce({ deviceId: "dev-a" });

    const staleBoot = boot();
    await vi.waitFor(() => expect(refreshDevices).toHaveBeenCalledTimes(2));
    await boot();
    expect(document.body.classList.contains("gated")).toBe(false);

    finishStaleRefresh(devices);
    await staleBoot;

    expect(document.body.classList.contains("gated")).toBe(false);
    expect(document.getElementById("root").textContent).not.toContain("Waiting for your device");
    const { App } = await import("../src/app.js");
    expect(App._watch).toBe(null);
  });

  it("keeps the newer boot responsible for waiting when a shared connection attempt fails", async () => {
    devices = [{ id: "dev-a", name: "Studio", fingerprint: "AAAA", status: "online" }];
    let rejectConnection;
    openSession = vi.fn(
      () =>
        new Promise((resolve, reject) => {
          rejectConnection = reject;
        }),
    );

    const olderBoot = boot();
    await vi.waitFor(() => expect(openSession).toHaveBeenCalledTimes(1));
    const newerBoot = boot();
    await vi.waitFor(() => expect(refreshDevices).toHaveBeenCalledTimes(2));
    rejectConnection(new Error("relay warming up"));
    await Promise.all([olderBoot, newerBoot]);

    expect(openSession).toHaveBeenCalledTimes(1);
    expect(document.getElementById("root").textContent).toContain("Waiting for your device");
    const { App } = await import("../src/app.js");
    expect(App._watch).not.toBe(null);
    clearInterval(App._watch);
    App._watch = null;
  });

  it("shares a successful connection attempt between overlapping boots", async () => {
    devices = [{ id: "dev-a", name: "Studio", fingerprint: "AAAA", status: "online" }];
    let finishConnection;
    openSession = vi.fn(
      () =>
        new Promise((resolve) => {
          finishConnection = resolve;
        }),
    );

    const olderBoot = boot();
    await vi.waitFor(() => expect(openSession).toHaveBeenCalledTimes(1));
    const newerBoot = boot();
    await vi.waitFor(() => expect(refreshDevices).toHaveBeenCalledTimes(2));
    const session = { deviceId: "dev-a" };
    finishConnection(session);
    await Promise.all([olderBoot, newerBoot]);

    expect(openSession).toHaveBeenCalledTimes(1);
    expect(openDeviceSessions).toHaveBeenCalledTimes(1);
    expect(render).toHaveBeenCalledTimes(1);
    expect(document.body.classList.contains("gated")).toBe(false);
  });
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
