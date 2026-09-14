// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderAppBehindBridgeGate, renderBridgeBehindAppGate } from "../src/views/versionGate.js";

let root;

beforeEach(() => {
  document.body.innerHTML = '<div id="root"></div>';
  root = document.querySelector("#root");
});

describe("the app-is-behind gate", () => {
  it("names the device and offers the reload the version watcher found", () => {
    const onReload = vi.fn();
    renderAppBehindBridgeGate(root, { deviceName: "ada's laptop", bridgeVersion: "2.0.0", onReload });
    expect(root.textContent).toContain("This app is behind the bridge on ada's laptop");
    expect(root.textContent).toContain("2.0.0");
    root.querySelector("#gate-reload").click();
    expect(onReload).toHaveBeenCalledTimes(1);
  });

  it("escapes a device name and hides the reload when there is nothing newer served", () => {
    renderAppBehindBridgeGate(root, { deviceName: "<img src=x>", bridgeVersion: "2.0.0", onReload: null });
    expect(root.querySelector("img")).toBe(null);
    expect(root.textContent).toContain("<img src=x>");
    expect(root.querySelector("#gate-reload")).toBe(null);
  });
});

describe("the bridge-is-behind gate", () => {
  it("names the device and shows the install command", () => {
    renderBridgeBehindAppGate(root, {
      deviceName: "ada's laptop",
      bridgeVersion: "1.1.0",
      installCommand: "curl -fsSL https://build.sh | sh",
    });
    expect(root.textContent).toContain("The bridge on ada's laptop needs updating");
    expect(root.textContent).toContain("curl -fsSL https://build.sh | sh");
  });

  it("copies the install command to the clipboard it is handed", async () => {
    const clipboard = { writeText: vi.fn(async () => {}) };
    renderBridgeBehindAppGate(root, {
      deviceName: "box",
      bridgeVersion: "1.1.0",
      installCommand: "install me",
      clipboard,
    });
    root.querySelector("#gate-install-copy").click();
    await Promise.resolve();
    expect(clipboard.writeText).toHaveBeenCalledWith("install me");
  });

  it("says the command is coming when there is none yet, with no copy button", () => {
    renderBridgeBehindAppGate(root, { deviceName: "box", bridgeVersion: "1.1.0", installCommand: "" });
    expect(root.querySelector("#gate-install-copy")).toBe(null);
    expect(root.textContent).toContain("box");
  });
});
