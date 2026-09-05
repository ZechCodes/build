// @vitest-environment jsdom
// Mounting the downloads block into a host that already holds other content:
// the placeholder is what ships in the html, the api's answer replaces it, and
// a refusal degrades to one line without taking the rest of the screen with it.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { downloadsPlaceholderHtml, mountDownloads } from "../src/core/downloads.js";

const DOWNLOADS = {
  install_command: "curl -fsSL https://getbuild.ing/install.sh | sh",
  install_script_url: "https://getbuild.ing/install.sh",
  releases_url: "https://github.com/ZechCodes/build-releases/releases/latest",
  checksums_url: "https://github.com/ZechCodes/build-releases/releases/latest/download/SHA256SUMS",
  platforms: [
    {
      key: "macos-arm64",
      label: "macOS · Apple silicon",
      url: "https://github.com/ZechCodes/build-releases/releases/latest/download/build-bridge-macos-arm64.tar.gz",
    },
  ],
};

const flush = () => new Promise((done) => setTimeout(done, 0));
const host = () => document.getElementById("root");

beforeEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = `<main id="root"><p id="pairing">Enter its pairing code</p>${downloadsPlaceholderHtml()}</main>`;
});

describe("mountDownloads", () => {
  it("paints the api's answer into the placeholder slot", async () => {
    await mountDownloads(host(), { fetchDownloads: async () => DOWNLOADS, platformKey: "macos-arm64" });
    await flush();
    const primary = document.querySelector("#downloads a.btn.primary");
    expect(primary.getAttribute("href")).toBe(DOWNLOADS.platforms[0].url);
    expect(document.getElementById("pairing").textContent).toBe("Enter its pairing code");
    expect(document.getElementById("downloadserr").textContent).toBe("");
  });

  it("copies the install command to the clipboard it was given and says so", async () => {
    vi.useFakeTimers();
    const clipboard = { writeText: vi.fn() };
    const mounted = mountDownloads(host(), {
      fetchDownloads: async () => DOWNLOADS,
      platformKey: "macos-arm64",
      clipboard,
    });
    await mounted;
    const button = document.getElementById("copycmd");
    button.click();
    expect(clipboard.writeText).toHaveBeenCalledWith(DOWNLOADS.install_command);
    expect(button.textContent).toBe("Copied");
    vi.advanceTimersByTime(1500);
    expect(button.textContent).toBe("Copy");
  });

  it("degrades to one line when the api refuses, leaving the rest of the screen alone", async () => {
    await mountDownloads(host(), {
      fetchDownloads: async () => {
        throw new Error("invite only");
      },
      platformKey: "macos-arm64",
    });
    await flush();
    expect(document.getElementById("downloadserr").textContent).toBe("invite only");
    expect(document.getElementById("downloads").textContent).toContain("aren't available right now");
    expect(document.getElementById("pairing").textContent).toBe("Enter its pairing code");
  });

  it("does nothing at all in a host that has no downloads slot", async () => {
    document.body.innerHTML = '<main id="root"><p id="pairing">Enter its pairing code</p></main>';
    const fetchDownloads = vi.fn();
    await mountDownloads(host(), { fetchDownloads, platformKey: "macos-arm64" });
    expect(fetchDownloads).not.toHaveBeenCalled();
  });
});
