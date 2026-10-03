import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const sourceRoot = fileURLToPath(new URL("../src/", import.meta.url));
const sourceFiles = readdirSync(sourceRoot, { recursive: true }).filter((file) => /\.(?:css|js)$/.test(file));
const start = "/* Shared select controls */";
const end = "/* End shared select controls */";
const styles = readFileSync(`${sourceRoot}styles.css`, "utf8");
const sharedBlock = () => styles.slice(styles.indexOf(start), styles.indexOf(end) + end.length);
const sharedProperty = /(?:^|;)\s*(?:appearance|-webkit-appearance|color|background(?:-[\w-]+)?|border(?:-(?:color|style|width|radius|top|right|bottom|left)(?:-[\w-]+)?)?|box-shadow|outline(?:-[\w-]+)?|font(?:-[\w-]+)?|padding(?:-[\w-]+)?|(?:min-|max-)?height)\s*:/;
const nativeSelector = /(?:^|[^\w-])select(?:[^\w-]|$)|::picker\(|::picker-icon|::checkmark|\.diffsort-select\b/;

function selectStyleViolations(file, source) {
  const violations = [];
  const outsideShared = file === "styles.css" ? source.replace(sharedBlock(), "") : source;
  if (file.endsWith(".css")) {
    const css = outsideShared.replace(/\/\*[\s\S]*?\*\//g, "");
    for (const [, selector, declarations] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      if (selector.trim() === ":where(button, input, select, textarea, [tabindex]):focus-visible") continue;
      if (nativeSelector.test(selector) && sharedProperty.test(declarations)) violations.push(`${file}: ${selector.trim()}`);
    }
  } else {
    for (const [markup] of source.matchAll(/<select\b[^>]*\bstyle\s*=\s*["'][^"']*["']/g)) {
      const inline = markup.match(/\bstyle\s*=\s*["']([^"']*)/)[1];
      if (sharedProperty.test(inline)) violations.push(`${file}: ${markup}`);
    }
  }
  return violations;
}

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

  it("keeps select paint, typography and vertical sizing out of surface CSS and inline styles", () => {
    const violations = sourceFiles.flatMap((file) => selectStyleViolations(file, readFileSync(`${sourceRoot}${file}`, "utf8")));
    expect(violations, "Select paint, typography and vertical sizing belong in the shared block").toEqual([]);
  });

  it.each([
    "color:red", "border:0", "background:transparent", "font:12px monospace", "font-size:11px",
    "padding:0", "padding-inline:2px", "padding-block-start:3px", "height:20px", "min-height:16px", "max-height:28px",
  ])("rejects a surface or inline select mutation adding %s", (declaration) => {
    expect(selectStyleViolations("styles/surface.css", `.surface select { ${declaration}; }`)).toHaveLength(1);
    expect(selectStyleViolations("core/surface.js", `<select style="${declaration}"><option>First</option></select>`)).toHaveLength(1);
    expect(selectStyleViolations("styles.css", `${styles}\nselect { ${declaration}; }`)).toHaveLength(1);
  });

  it("continues to allow surface width, margin and placement rules", () => {
    expect(selectStyleViolations("styles/surface.css", ".surface select { width:100%; max-width:340px; min-width:0; margin:4px; align-self:start; }")).toEqual([]);
    expect(selectStyleViolations("core/surface.js", '<select style="width:100%;margin-left:auto"><option>First</option></select>')).toEqual([]);
  });
});
