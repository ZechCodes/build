// Which build to offer this browser. The browser cannot tell Apple silicon
// from Intel on its own — every Mac reports "MacIntel"/"Intel Mac OS X",
// Rosetta included — so the only Intel evidence worth trusting is
// userAgentData.architecture. A Mac with no architecture hint is Apple silicon.

import { describe, it, expect, vi, afterEach } from "vitest";
import { platformKeyFor, currentPlatformKey } from "../src/core/platform.js";

const MAC_SAFARI =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";
const MAC_CHROME =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const LINUX_FIREFOX = "Mozilla/5.0 (X11; Linux x86_64; rv:126.0) Gecko/20100101 Firefox/126.0";
const LINUX_ARM_CHROMIUM =
  "Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";
const WINDOWS_EDGE =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 Edg/126.0.0.0";
const IPHONE_SAFARI =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1";

afterEach(() => vi.unstubAllGlobals());

describe("platformKeyFor", () => {
  it("reads a Mac with no architecture hint as Apple silicon", () => {
    expect(platformKeyFor({ platform: "MacIntel", userAgent: MAC_SAFARI })).toBe("macos-arm64");
    expect(platformKeyFor({ platform: "macOS", userAgent: MAC_CHROME })).toBe("macos-arm64");
  });

  it("reads a Mac as Intel only when userAgentData says the architecture is x86", () => {
    expect(platformKeyFor({ platform: "macOS", userAgent: MAC_CHROME, architecture: "x86" })).toBe("macos-x86_64");
  });

  it("reads Intel only from userAgentData.architecture, never from the shape's text", () => {
    expect(platformKeyFor({ platform: "MacIntel x86_64", userAgent: MAC_SAFARI })).toBe("macos-arm64");
    expect(platformKeyFor({ platform: "MacIntel x86_64", userAgent: MAC_SAFARI, architecture: "arm" })).toBe(
      "macos-arm64",
    );
  });

  it("reads Linux as x86_64 unless the shape names arm", () => {
    expect(platformKeyFor({ platform: "Linux x86_64", userAgent: LINUX_FIREFOX })).toBe("linux-x86_64");
    expect(platformKeyFor({ platform: "Linux aarch64", userAgent: LINUX_ARM_CHROMIUM })).toBe("linux-aarch64");
  });

  it("offers nothing for a platform with no bridge build", () => {
    expect(platformKeyFor({ platform: "Win32", userAgent: WINDOWS_EDGE })).toBe(null);
    expect(platformKeyFor({ platform: "iPhone", userAgent: IPHONE_SAFARI })).toBe(null);
    expect(platformKeyFor({ platform: "", userAgent: "" })).toBe(null);
    expect(platformKeyFor()).toBe(null);
  });
});

describe("currentPlatformKey", () => {
  it("prefers userAgentData over the frozen navigator.platform string", () => {
    vi.stubGlobal("navigator", {
      platform: "MacIntel",
      userAgent: MAC_CHROME,
      userAgentData: { platform: "macOS", architecture: "x86" },
    });
    expect(currentPlatformKey()).toBe("macos-x86_64");
  });

  it("falls back to navigator.platform where userAgentData is absent", () => {
    vi.stubGlobal("navigator", { platform: "Linux aarch64", userAgent: LINUX_ARM_CHROMIUM });
    expect(currentPlatformKey()).toBe("linux-aarch64");
  });
});
