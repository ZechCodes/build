// @vitest-environment jsdom
// Mounting the downloads block into a host that already holds other content:
// the placeholder is what ships in the html, the api's answer replaces it, and
// a refusal degrades to one line without taking the rest of the screen with it.
//
// The one-liner the api paints carries a token that lives ten minutes, so Copy
// is where the freshness rule lives: a line the human is copying a minute or
// more after it was painted is re-minted first, and what lands on the clipboard
// is what is on screen.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { downloadsPlaceholderHtml, mountDownloads } from "../src/core/downloads.js";
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

beforeEach(() => {
  vi.useRealTimers();
  clock = 1_000_000;
  clipboard = { writeText: vi.fn() };
  mintInstallCommand = vi.fn(async () => ({ install_command: mintedCommand(), expires_in_s: 600 }));
  document.body.innerHTML = `<main id="root"><p id="pairing">Enter its pairing code</p>${downloadsPlaceholderHtml()}</main>`;
});

describe("mountDownloads", () => {
  it("paints the api's answer, one-liner and all, into the placeholder slot", async () => {
    await mount();
    await flush();
    const primary = document.querySelector("#downloads a.btn.primary");
    expect(primary.getAttribute("href")).toBe(DOWNLOADS.platforms[0].url);
    expect(document.getElementById("installcmd").textContent).toBe(DOWNLOADS.install_command);
    expect(document.getElementById("pairing").textContent).toBe("Enter its pairing code");
    expect(document.getElementById("downloadserr").textContent).toBe("");
  });

  it("copies the line on screen while it is still fresh, without spending a mint", async () => {
    await mount();
    clock += MINUTE;
    await copy();
    expect(mintInstallCommand).not.toHaveBeenCalled();
    expect(clipboard.writeText).toHaveBeenCalledWith(DOWNLOADS.install_command);
  });

  it("re-mints a line older than a minute, repaints it, and copies the fresh one", async () => {
    await mount();
    clock += MINUTE + 1;
    await copy();
    expect(mintInstallCommand).toHaveBeenCalledTimes(1);
    expect(document.getElementById("installcmd").textContent).toBe(mintedCommand());
    expect(clipboard.writeText).toHaveBeenCalledWith(mintedCommand());
  });

  it("counts the age from the last mint, so a second Copy right after is free", async () => {
    await mount();
    clock += MINUTE + 1;
    await copy();
    await copy();
    expect(mintInstallCommand).toHaveBeenCalledTimes(1);
    expect(clipboard.writeText).toHaveBeenLastCalledWith(mintedCommand());
  });

  it("copies the line on screen when the re-mint refuses — the api is the one judge", async () => {
    mintInstallCommand = vi.fn(async () => {
      throw new Error("invite only");
    });
    await mount();
    clock += MINUTE + 1;
    await copy();
    expect(document.getElementById("installcmd").textContent).toBe(DOWNLOADS.install_command);
    expect(clipboard.writeText).toHaveBeenCalledWith(DOWNLOADS.install_command);
  });

  it("says Copied for a moment, then goes back to Copy", async () => {
    vi.useFakeTimers();
    await mount();
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
