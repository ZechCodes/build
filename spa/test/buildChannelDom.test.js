// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// Keep startup local: boot and browser services are isolated, while main.js,
// the real config and badge initializer execute together.
vi.mock("../src/app.js", () => ({ App: {}, initRouter: vi.fn(), go: vi.fn(), followNotificationLink: vi.fn() }));
vi.mock("../src/devices.js", () => ({ initDevicePicker: vi.fn() }));
vi.mock("../src/push.js", () => ({ installPushOpenListener: vi.fn(), registerPushWorker: vi.fn() }));
vi.mock("../src/core/pushKeySync.js", () => ({ startPushKeySync: vi.fn() }));
vi.mock("../src/core/version.js", () => ({ createVersionWatcher: vi.fn(), fetchServedVersion: vi.fn() }));
vi.mock("../src/core/sheetDismiss.js", () => ({ requestSheetDismiss: vi.fn() }));
vi.mock("../src/core/theme.js", () => ({ installTheme: vi.fn() }));
vi.mock("../src/core/composeView.js", () => ({ initCompose: vi.fn() }));
vi.mock("../src/views/gate.js", () => ({ boot: vi.fn(), openPairingLink: vi.fn() }));
vi.mock("../src/core/pairLink.js", () => ({ watchPairLinks: vi.fn() }));
vi.mock("../src/connectionStatus.js", () => ({ mountConnectionStatus: vi.fn() }));
vi.mock("../src/core/userPresence.js", () => ({ startUserPresence: vi.fn() }));
vi.mock("../src/core/windowControlsOverlay.js", () => ({ installWindowControlsOverlay: vi.fn() }));
vi.mock("../src/core/markdownNavigation.js", () => ({ installMarkdownNavigation: vi.fn() }));
vi.mock("../src/core/markdownTableFocus.js", () => ({ installMarkdownTableFocus: vi.fn() }));
vi.mock("../src/core/markdownImages.js", () => ({ installMarkdownImages: vi.fn() }));

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
  it("initializes the Dev identity from main before the gate boots", async () => {
    vi.stubEnv("VITE_BUILD_CHANNEL", "dev");
    const { boot } = await import("../src/views/gate.js");
    boot.mockImplementationOnce(() => {
      expect(document.querySelector("#build-channel")).toBeNull();
      expect(document.title).toBe("Build Dev — review is the product");
    });
    await import("../src/main.js");
    expect(boot).toHaveBeenCalledOnce();
  });

  it("adds a compact Dev identity to the connection button", async () => {
    await initialize("dev");
    expect((await import("../src/config.js")).BUILD_CHANNEL).toBe("dev");
    const { mountBuildChannel } = await import("../src/core/buildChannel.js");
    const button = document.createElement("button");
    mountBuildChannel(button);
    const badge = button.querySelector(".connection-build-channel");
    expect(badge.textContent).toBe("D");
    expect(badge.getAttribute("aria-hidden")).toBe("true");
    expect(button.getAttribute("aria-description")).toBe("Dev build");
    expect(document.querySelector("#build-channel")).toBeNull();
    expect(document.title).toBe("Build Dev — review is the product");
  });

  it.each([undefined, "", "prod", "preview"])("keeps the original UI/title for %s", async (channel) => {
    await initialize(channel);
    const { mountBuildChannel } = await import("../src/core/buildChannel.js");
    const button = document.createElement("button");
    mountBuildChannel(button);
    expect(button.children).toHaveLength(0);
    expect(button.hasAttribute("aria-description")).toBe(false);
    expect(document.title).toBe(originalTitle);
  });

  it("keeps build identity out of the header on welcome and empty devices", async () => {
    await initialize("dev");
    document.body.classList.add("gated");
    document.querySelector("#root").innerHTML = "<h1>Welcome to Build</h1>";
    expect(document.querySelector("#build-channel")).toBeNull();
    expect(document.title).toBe("Build Dev — review is the product");
  });
});
