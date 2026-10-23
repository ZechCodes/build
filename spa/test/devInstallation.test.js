// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { downloadsHtml, downloadsPlaceholderHtml, mountDownloads } from "../src/core/downloads.js";
import { renderBridgeBehindAppGate } from "../src/views/versionGate.js";
import { mountBridgeUpdatePanel } from "../src/core/bridgeUpdatePanel.js";
import { openAddDevice } from "../src/sheets/addDevice.js";
import { developmentPairingLateText } from "../src/core/developmentInstallation.js";
import { downloadsPayload } from "./downloadsFixture.js";

vi.mock("../src/config.js", async (original) => ({ ...await original(), BUILD_CHANNEL: "dev" }));

let host;
beforeEach(() => { host = document.createElement("div"); });

function expectSourceGuidance() {
  expect(host.textContent).toContain("Build the dev bridge from reviewed source");
  expect(host.textContent).toContain("separate authorization");
  expect(host.innerHTML).not.toMatch(/getbuild\.ing\/install|build-releases|install-service|curl /);
  expect(host.querySelectorAll("button, a[download]")).toHaveLength(0);
}

it("withholds stale production downloads and desktop installers in a dev bundle", () => {
  host.innerHTML = downloadsHtml(downloadsPayload(), "linux-x86_64");
  expectSourceGuidance();
});

it("shows source guidance before downloads load and does not request production metadata", async () => {
  host.innerHTML = downloadsPlaceholderHtml();
  expectSourceGuidance();
  const fetchDownloads = vi.fn(async () => downloadsPayload());
  await mountDownloads(host, { fetchDownloads });
  expect(fetchDownloads).not.toHaveBeenCalled();
  expectSourceGuidance();
});

it.each([undefined, downloadsPayload().install_command])("withholds the old bridge gate installer even with command %s", (installCommand) => {
  renderBridgeBehindAppGate(host, { deviceName: "Dev laptop", bridgeVersion: "0.9.0", installCommand });
  expectSourceGuidance();
});

it("withholds release update controls and RPCs on dev", () => {
  const callRpc = vi.fn(async () => null);
  const dispose = mountBridgeUpdatePanel(host, { deviceId: "dev", callRpc });
  expectSourceGuidance();
  expect(callRpc).not.toHaveBeenCalled();
  dispose();
});

it("keeps a delayed pairing notice from suggesting the production service", () => {
  host.textContent = developmentPairingLateText("Dev laptop");
  expectSourceGuidance();
});

it("warns pairing-link arrivals about a separately authorized source-built dev bridge", () => {
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  openAddDevice(vi.fn(), { fromLink: true });
  const warning = document.querySelector("[data-link-warning]").textContent;
  expect(warning).toContain("separately authorized");
  expect(warning).toContain("source-built dev bridge");
  expect(warning).not.toMatch(/installer|build-bridge pair/);
});
