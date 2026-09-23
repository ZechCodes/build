// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { downloadsPlaceholderHtml, mountDownloads } from "../src/core/downloads.js";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { downloadsAddress } from "../src/core/settingsRecords.js";
import { asset, downloadsPayload, mintedCommand } from "./downloadsFixture.js";

const DOWNLOADS = downloadsPayload({
  platforms: [{ key: "macos-arm64", label: "macOS · Apple silicon", url: asset("macos-arm64") }],
});
const MINUTE = 60_000;

const flush = () => new Promise((done) => setTimeout(done, 0));
const host = () => document.getElementById("root");

let clock;
let clipboard;
let mintInstallCommand;

const mount = (overrides = {}) =>
  mountDownloads(host(), {
    fetchDownloads: async () => DOWNLOADS,
    mintInstallCommand,
    platformKey: "macos-arm64",
    clipboard,
    now: () => clock,
    ...overrides,
  });

const copy = async () => {
  document.getElementById("copycmd").click();
  await flush();
};

beforeEach(async () => {
  vi.useRealTimers();
  await wipeCache();
  clock = 1_000_000;
  clipboard = { writeText: vi.fn() };
  mintInstallCommand = vi.fn(async () => ({ install_command: mintedCommand(), expires_in_s: 600 }));
  document.body.innerHTML = `<main id="root"><p id="pairing">Enter its pairing code</p>${downloadsPlaceholderHtml()}</main>`;
});

describe("mountDownloads", () => {
  it("paints cached download links while the API has no answer", async () => {
    await writeCached(downloadsAddress, DOWNLOADS);
    const fetchDownloads = vi.fn(() => new Promise(() => {}));
    void mount({ fetchDownloads });
    await vi.waitFor(() => expect(document.querySelector("#downloads a.btn.primary")?.getAttribute("href")).toBe(DOWNLOADS.platforms[0].url));
    await vi.waitFor(() => expect(fetchDownloads).toHaveBeenCalledOnce());
  });

  it("paints the api's answer, one-liner and all, into the placeholder slot", async () => {
    await mount();
    await flush();
    const primary = document.querySelector("#downloads a.btn.primary");
    expect(primary.getAttribute("href")).toBe(DOWNLOADS.platforms[0].url);
    expect(document.getElementById("installcmd").textContent).toBe(DOWNLOADS.install_command);
    expect(document.getElementById("pairing").textContent).toBe("Enter its pairing code");
    expect(document.getElementById("downloadserr").textContent).toBe("");
  });

  it("copies the public bridge line even after the page has been open for hours", async () => {
    await mount();
    clock += 180 * MINUTE;
    await copy();
    expect(mintInstallCommand).not.toHaveBeenCalled();
    expect(clipboard.writeText).toHaveBeenCalledWith(DOWNLOADS.install_command);
  });

  it("copies the desktop installer independently of the bridge", async () => {
    await mount();
    document.getElementById("desktopcopycmd").click();
    await flush();
    expect(clipboard.writeText).toHaveBeenCalledWith(DOWNLOADS.desktop_install_command);
    expect(mintInstallCommand).not.toHaveBeenCalled();
  });

  it("says Copied for a moment, then goes back to Copy", async () => {
    await mount();
    vi.useFakeTimers();
    const button = document.getElementById("copycmd");
    button.click();
    await vi.advanceTimersByTimeAsync(0);
    expect(button.textContent).toBe("Copied");
    await vi.advanceTimersByTimeAsync(1500);
    expect(button.textContent).toBe("Copy");
  });

  it("degrades to one line when the api refuses, leaving the rest of the screen alone", async () => {
    await mount({
      fetchDownloads: async () => {
        throw new Error("invite only");
      },
    });
    await flush();
    expect(document.getElementById("downloadserr").textContent).toBe("invite only");
    expect(document.getElementById("downloads").textContent).toContain("aren't available right now");
    expect(document.getElementById("pairing").textContent).toBe("Enter its pairing code");
  });

  it("does nothing at all in a host that has no downloads slot", async () => {
    document.body.innerHTML = '<main id="root"><p id="pairing">Enter its pairing code</p></main>';
    const fetchDownloads = vi.fn();
    await mount({ fetchDownloads });
    expect(fetchDownloads).not.toHaveBeenCalled();
  });
});
