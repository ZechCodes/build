// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { identityBadgeHtml, rowIdentityBadgeHtml } from "../src/core/environmentIdentity.js";
import { deviceKey } from "../src/core/deviceKey.js";

const badge = (name, device = "laptop", environment = "env-1", kind = "project") => {
  const host = document.createElement("div");
  host.innerHTML = identityBadgeHtml({ key: deviceKey(device, environment), name }, kind);
  return host.firstElementChild;
};

describe("identityBadgeHtml", () => {
  it("keeps decorative glyph letters out of adjacent labels' DOM text", () => {
    for (const kind of ["environment", "project", "workspace"]) {
      const host = document.createElement("button");
      host.innerHTML = identityBadgeHtml({ key:"laptop/env-1", name:"Alpha" }, kind) + "Alpha";
      expect(host.firstElementChild.getAttribute("aria-hidden")).toBe("true");
      expect(host.firstElementChild.textContent).toBe("");
      expect(host.textContent).toBe("Alpha");
    }
  });

  it("keeps the same device/environment hue after module reload and name changes", async () => {
    const first = badge("Alpha");
    vi.resetModules();
    const reloaded = await import("../src/core/environmentIdentity.js");
    expect(reloaded.identityBadgeHtml({ key: "laptop/env-1", name: "Alpha" }, "project")).toBe(first.outerHTML);
    expect(badge("Renamed").dataset.hue).toBe(first.dataset.hue);
    expect(Number(first.dataset.hue)).toBeGreaterThanOrEqual(1);
    expect(Number(first.dataset.hue)).toBeLessThanOrEqual(12);
    expect(first.getAttribute("style")).toBe(`--identity-hue:var(--badge-${first.dataset.hue})`);
  });

  it("gives siblings the same hue and their own glyph", () => {
    const siblings = [badge("Alpha"), badge("Beta"), badge("Sources", "laptop", "env-1", "environment"), badge("Checkout", "laptop", "env-1", "workspace")];
    expect(new Set(siblings.map(row => row.dataset.hue)).size).toBe(1);
    expect(siblings.map(row => row.querySelector("[data-glyph]").dataset.glyph)).toEqual(["AL", "BE", "SO", "CH"]);
    for (const keys of [Array.from({ length: 24 }, (_, i) => badge("Alpha", `device-${i}`)), Array.from({ length: 24 }, (_, i) => badge("Alpha", "laptop", `env-${i}`))]) {
      expect(new Set(keys.map(row => row.dataset.hue)).size).toBeGreaterThan(1);
    }
  });

  it("uses a legacy project id when no environment exists", () => {
    expect(rowIdentityBadgeHtml({ deviceId: "laptop", project_id: "proj-1", name: "Legacy" }, "project"))
      .toBe(identityBadgeHtml({ key: "laptop/proj-1", name: "Legacy" }));
  });

  it.each([["alpha beta", "AL"], [" a ", "A"], ["123", "?"], ["🛠", "?"], ["", "?"], ["123 café", "CA"], ["文档", "文档"], ["ßa", "ßA"], ["ß", "ß"]])("selects two source letters before uppercasing each without expansion: %j", (name, expected) => {
    expect(badge(name).querySelector("[data-glyph]").dataset.glyph).toBe(expected);
  });

  it("escapes untrusted names and attributes inside the existing hidden 16px SVG slot", () => {
    const row = badge('<img src=x onerror="alert(1)">', '"/><script>', 'env', '\" onload=\"alert(1)');
    expect(row.tagName.toLowerCase()).toBe("svg");
    expect(row.classList.contains("identity-badge")).toBe(true);
    expect(row.getAttribute("width")).toBe("16");
    expect(row.getAttribute("height")).toBe("16");
    expect(row.getAttribute("aria-hidden")).toBe("true");
    expect(row.getAttribute("focusable")).toBe("false");
    expect(row.querySelector("img,script")).toBeNull();
    expect(row.hasAttribute("onload")).toBe(false);
    expect(row.dataset.kind).toBe('\" onload=\"alert(1)');
  });
});
