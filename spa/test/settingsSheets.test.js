import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { settingsSheetHtml } from "../src/sheets/settingsSheet.js";

describe("settings sheet frame", () => {
  it("escapes its fixed title and separates it from the body", () => {
    const html = settingsSheetHtml({ title: "<Settings>", bodyHtml: '<input id="control">' });
    expect(html).toContain("&lt;Settings&gt;");
    expect(html).toMatch(/settings-sheet-header[\s\S]*<\/header>\s*<div class="settings-sheet-body">/);
  });

  it("pins the header while only the body scrolls", () => {
    const css = readFileSync(resolve("src/styles/settingsSheets.css"), "utf8");
    expect(css).toMatch(/\.sheet:has\(> \.settings-sheet-frame\)\s*\{[^}]*overflow:hidden/);
    expect(css).toMatch(/\.settings-sheet-header\s*\{[^}]*flex:none/);
    expect(css).toMatch(/\.settings-sheet-body\s*\{[^}]*overflow-y:auto/);
    expect(css.match(/overflow-y:auto/g)).toHaveLength(1);
  });
});
