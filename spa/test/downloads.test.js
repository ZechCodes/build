// @vitest-environment jsdom
// The downloads block, as pure html. One renderer serves both hosts — the
// first-run gate and Settings — so the copy and the links have one home.

import { describe, it, expect } from "vitest";
import { downloadsHtml } from "../src/core/downloads.js";
import { asset, downloadsPayload } from "./downloadsFixture.js";

const DOWNLOADS = downloadsPayload();

const parse = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
};
const linkUrls = (host) => [...host.querySelectorAll("a")].map((a) => a.getAttribute("href"));

describe("downloadsHtml", () => {
  it("makes the detected platform the one primary button, carrying the api's url", () => {
    const host = parse(downloadsHtml(DOWNLOADS, "linux-aarch64"));
    const primary = host.querySelectorAll("a.btn.primary");
    expect(primary).toHaveLength(1);
    expect(primary[0].getAttribute("href")).toBe(asset("linux-aarch64"));
    expect(primary[0].textContent).toBe("Download for Linux · arm64");
  });

  it("offers all four as plain links, in the payload's order, when the platform is unknown", () => {
    const host = parse(downloadsHtml(DOWNLOADS, null));
    expect(host.querySelectorAll("a.btn.primary")).toHaveLength(0);
    const offered = [...host.querySelectorAll("a")].filter((a) => a.textContent.startsWith("macOS") || a.textContent.startsWith("Linux"));
    expect(offered.map((a) => a.textContent)).toEqual([
      "macOS · Apple silicon",
      "macOS · Intel",
      "Linux · x86_64",
      "Linux · arm64",
    ]);
    expect(offered.map((a) => a.getAttribute("href"))).toEqual(DOWNLOADS.platforms.map((p) => p.url));
  });

  it("renders the install command verbatim, next to a Copy button", () => {
    const host = parse(downloadsHtml(DOWNLOADS, "macos-arm64"));
    expect(host.querySelector("#installcmd").textContent).toBe("curl -fsSL https://getbuild.ing/install.sh | sh");
    expect(host.querySelector("#copycmd").textContent).toBe("Copy");
  });

  it("keeps the other three platforms reachable beside the checksums and the releases page", () => {
    const host = parse(downloadsHtml(DOWNLOADS, "macos-arm64"));
    const urls = linkUrls(host);
    expect(urls).toContain(asset("macos-x86_64"));
    expect(urls).toContain(asset("linux-x86_64"));
    expect(urls).toContain(asset("linux-aarch64"));
    expect(urls).toContain(DOWNLOADS.checksums_url);
    expect(urls).toContain(DOWNLOADS.releases_url);
  });

  it("says where the bridge runs and what the servers carry", () => {
    const text = parse(downloadsHtml(DOWNLOADS, "macos-arm64")).textContent;
    expect(text).toContain("the bridge runs on your machine");
    expect(text).toContain("Build's servers move ciphertext");
  });

  it("escapes every label, url and command the api hands it", () => {
    const hostile = {
      ...DOWNLOADS,
      install_command: '"><img src=x onerror=alert(1)>',
      checksums_url: '"><img src=x>',
      releases_url: '"><img src=x>',
      platforms: [{ key: "macos-arm64", label: '"><img src=x>', url: '"><img src=x>' }],
    };
    const host = parse(downloadsHtml(hostile, "macos-arm64"));
    expect(host.querySelectorAll("img")).toHaveLength(0);
    expect(host.querySelector("#installcmd").textContent).toBe('"><img src=x onerror=alert(1)>');
  });

  it("survives a payload with no platforms rather than throwing at the gate", () => {
    const host = parse(downloadsHtml({ install_command: "curl | sh", platforms: undefined }, "macos-arm64"));
    expect(host.querySelector("#installcmd").textContent).toBe("curl | sh");
  });
});
