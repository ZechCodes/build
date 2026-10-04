import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const root = fileURLToPath(new URL("../src/", import.meta.url));
const files = readdirSync(root, { recursive: true }).filter((file) => /\.(?:css|js)$/.test(file));
const styles = readFileSync(`${root}styles.css`, "utf8");
const start = "/* Shared text controls */";
const end = "/* End shared text controls */";
const sharedBlock = () => styles.slice(styles.indexOf(start), styles.indexOf(end) + end.length);
const paint = /(?:^|;)\s*(?:appearance|-webkit-appearance|color|caret-color|line-height|background(?:-[\w-]+)?|border(?:-[\w-]+)?|box-shadow|outline(?:-[\w-]+)?|font(?:-[\w-]+)?|padding(?:-[\w-]+)?)\s*:/;
const fieldSelector = /(?:^|[^\w-])(?:input|textarea)(?:[^\w-]|$)|\.(?:cp-input|csinput|file-editor|fmenu-search|task-compose-title|workspace-refsearch|tb-filter)(?![\w-])/;
const inputSelector = /(?:^|[^\w-])input(?:[^\w-]|$)|\.(?:fmenu-search|task-compose-title|workspace-refsearch|tb-filter)(?![\w-])/;
const verticalSize = /(?:^|;)\s*(?:min-|max-)?height\s*:/;
const nonTextSelector = /input\[type\s*=\s*["']?(?:checkbox|radio|range|file|hidden|button|submit|reset|color|image)["']?\]|\.menu-slider\b/;

function inputHeights(block) {
  return [...block.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter(([, selector]) => inputSelector.test(selector))
    .flatMap(([, , declarations]) => [...declarations.matchAll(/(?:^|;)\s*((?:min-|max-)?height)\s*:\s*([^;]+);/g)]
      .map(([, property, value]) => `${property}:${value.trim()}`));
}

function violations(file, source) {
  const outside = file === "styles.css" ? source.replace(sharedBlock(), "") : source;
  if (file.endsWith(".css")) {
    return [...outside.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)]
      .filter(([, selectors, declarations]) => selectors.trim() !== ":where(button, input, select, textarea, [tabindex]):focus-visible" && selectors.split(",").some((selector) =>
        (paint.test(declarations) && fieldSelector.test(selector) || verticalSize.test(declarations) && inputSelector.test(selector)) && !nonTextSelector.test(selector)))
      .map(([, selector]) => `${file}: ${selector.trim()}`);
  }
  return [...source.matchAll(/<(?:input|textarea)\b[^>]*\bstyle\s*=\s*["']([^"']*)["']/g)]
    .filter(([tag, inline]) => !/type=["'](?:checkbox|radio|range|file|hidden|button|submit|reset|color|image)["']/.test(tag) && (paint.test(inline) || tag.startsWith("<input") && verticalSize.test(inline)))
    .map(([tag]) => `${file}: ${tag}`);
}

describe("shared text field styling", () => {
  it("owns text-like inputs, textareas and their field states in one token-based block", () => {
    expect(styles.split(start)).toHaveLength(2);
    expect(styles.split(end)).toHaveLength(2);
    const block = sharedBlock();
    for (const token of ["--select-height", "--select-compact-height", "--select-radius", "--line", "--panel", "--bg", "--ink", "--ink2", "--dim", "--accent", "--focus-ring"]) {
      expect(block, token).toContain(`var(${token})`);
    }
    for (const state of [":hover", ":focus-visible", ":disabled", ":read-only", "::placeholder"]) expect(block).toContain(state);
    expect(block).toContain("textarea");
    expect(block).not.toMatch(/#[\da-f]{3,8}\b|\brgba?\(/i);
  });

  it("permits exactly the standard and compact select heights for single-line fields", () => {
    expect(inputHeights(sharedBlock())).toEqual(["height:var(--select-height)", "height:var(--select-compact-height)"]);
    expect(sharedBlock()).toMatch(/input\.mini\s*\{[^}]*font-size:12px/);
    expect(inputHeights(`${sharedBlock()}\ninput.extra { height:30px; }`)).toContain("height:30px");
  });

  it("scopes muted read-only paint to enabled text fields and excludes it from hover", () => {
    const rule = sharedBlock().match(/([^{}]+:read-only:not\(:disabled\))\s*\{([^{}]+)\}/);
    expect(rule).not.toBeNull();
    for (const type of ["checkbox", "radio", "file", "hidden", "button", "submit", "reset", "range", "color", "image"]) {
      expect(rule[1]).toContain(`[type=${type}]`);
    }
    expect(rule[2]).toContain("color:var(--ink2)");
    expect(rule[2]).toContain("cursor:default");
    expect(sharedBlock()).toContain(":hover:not(:disabled, :read-only)");
  });

  it("keeps input and textarea paint out of surface and inline rules", () => {
    expect(files.flatMap((file) => violations(file, readFileSync(`${root}${file}`, "utf8")))).toEqual([]);
  });

  it.each(["color:red", "background:transparent", "border:0", "border-radius:3px", "font:12px monospace", "font-size:11px", "padding:0", "box-shadow:none", "outline:none", "caret-color:red", "line-height:1"])("rejects a second field style adding %s", (declaration) => {
    for (const selector of [".surface input", ".surface textarea", ".tb-filter", ".file-editor"]) {
      expect(violations("styles/surface.css", `${selector} { ${declaration}; }`)).toHaveLength(1);
    }
    for (const tag of ["input", "textarea"]) expect(violations("core/surface.js", `<${tag} style="${declaration}">`)).toHaveLength(1);
    expect(violations("styles.css", `${styles}\ninput { ${declaration}; }`)).toContain("styles.css: input");
  });

  it.each(["height:20px", "min-height:16px", "max-height:28px"])("rejects a second single-line vertical metric adding %s", (declaration) => {
    for (const selector of [".surface input", ".tb-filter", ".workspace-refsearch", ".task-compose-title"]) {
      expect(violations("styles/surface.css", `${selector} { ${declaration}; }`)).toHaveLength(1);
    }
    expect(violations("core/surface.js", `<input style="${declaration}">`)).toHaveLength(1);
    expect(violations("core/surface.js", `<textarea style="${declaration}">`)).toEqual([]);
  });

  it("allows layout, autosizing, and separate non-text controls", () => {
    expect(violations("styles/surface.css", '.surface textarea { width:100%; min-height:72px; max-height:40vh; resize:none; overflow:auto; margin:4px; } input[type="checkbox"] { accent-color:var(--accent); } .menu-slider input { padding:0; }')).toEqual([]);
    expect(violations("core/surface.js", '<input style="width:100%"><textarea style="height:80px">')).toEqual([]);
  });
});
