import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const sourceRoot = fileURLToPath(new URL("../src/", import.meta.url));
const sourceFiles = readdirSync(sourceRoot, { recursive: true }).filter((file) => /\.(?:css|js)$/.test(file));
const start = "/* Shared select controls */";
const end = "/* End shared select controls */";
const styles = readFileSync(`${sourceRoot}styles.css`, "utf8");
const sharedBlock = () => styles.slice(styles.indexOf(start), styles.indexOf(end) + end.length);
const paintProperty = /(?:^|;)\s*(?:appearance|-webkit-appearance|color|background(?:-[\w-]+)?|border(?:-(?:color|style|width|radius|top|right|bottom|left)(?:-[\w-]+)?)?|box-shadow|outline(?:-[\w-]+)?)\s*:/;
const nativeSelector = /(?:^|[^\w-])select(?:[^\w-]|$)|::picker\(|::picker-icon|::checkmark|\.diffsort-select\b/;

describe("shared native select styling", () => {
  it("owns the base control, fallback chevron and Chromium picker in one marked block", () => {
    expect(styles.split(start)).toHaveLength(2);
    expect(styles.split(end)).toHaveLength(2);
    expect(styles.indexOf(end)).toBeGreaterThan(styles.indexOf(start));
    const block = sharedBlock();
    for (const selector of ["select", "::picker(select)", "::picker-icon", "option", "::checkmark"]) {
      expect(block, selector).toContain(selector);
    }
    expect(block).toMatch(/appearance:\s*base-select/);
    for (const token of ["--select-radius", "--select-height", "--select-compact-height", "--select-menu-radius"]) {
      expect(styles, token).toContain(token);
      expect(block, token).toContain(`var(${token})`);
    }
  });

  it("keeps select paint out of surface CSS and inline styles", () => {
    const violations = [];
    for (const file of sourceFiles) {
      const source = readFileSync(`${sourceRoot}${file}`, "utf8");
      const outsideShared = file === "styles.css" ? source.replace(sharedBlock(), "") : source;
      if (file.endsWith(".css")) {
        const css = outsideShared.replace(/\/\*[\s\S]*?\*\//g, "");
        for (const [, selector, declarations] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
          // The shared keyboard focus colour intentionally covers all controls.
          if (selector.trim() === ":where(button, input, select, textarea, [tabindex]):focus-visible") continue;
          if (nativeSelector.test(selector) && paintProperty.test(declarations)) {
            violations.push(`${file}: ${selector.trim()}`);
          }
        }
      } else {
        for (const [markup] of source.matchAll(/<select\b[^>]*\bstyle\s*=\s*["'][^"']*["']/g)) {
          const inline = markup.match(/\bstyle\s*=\s*["']([^"']*)/)[1];
          if (paintProperty.test(inline)) violations.push(`${file}: ${markup}`);
        }
      }
    }
    expect(violations, "Select colours, borders and picker paint belong in the shared block").toEqual([]);
  });
});
