import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const stylesSource = readFileSync(
  fileURLToPath(new URL("../src/styles.css", import.meta.url)),
  "utf8",
);

/** Every `selector { declarations }` pair in the sheet. Comments are stripped
 *  first and at-rule preludes never match (their inner rules do), so a rule
 *  inside a media query is read exactly like a top-level one. */
function cssRules() {
  const source = stylesSource.replace(/\/\*[\s\S]*?\*\//g, "");
  return [...source.matchAll(/([^{}@;]+)\{([^{}]*)\}/g)].map(([, selector, body]) => ({
    selector: selector.trim().replace(/\s+/g, " "),
    body: body.trim(),
  }));
}

const rulesFor = (selector) => cssRules().filter((rule) => rule.selector === selector);

describe("tab layout primitives", () => {
  it("declares the content width once, as a :root token", () => {
    expect(stylesSource.match(/--content-max:/g) || []).toHaveLength(1);
    const token = cssRules().find((rule) => rule.selector === ":root" && rule.body.includes("--content-max"));
    expect(token).toBeTruthy();
    expect(token.body).toMatch(/--content-max:\s*1180px/);
  });

  it("sizes both layouts from one shared width rule", () => {
    const widthRules = cssRules().filter(
      (rule) => /\.pane-(col|split)\b/.test(rule.selector) && /\bmax-width\s*:/.test(rule.body),
    );
    expect(widthRules).toHaveLength(1);
    const [shared] = widthRules;
    expect(shared.selector.split(",").map((part) => part.trim()).sort()).toEqual([".pane-col", ".pane-split"]);
    expect(shared.body).toMatch(/width:\s*100%/);
    expect(shared.body).toMatch(/max-width:\s*var\(--content-max\)/);
    expect(shared.body).toMatch(/margin:\s*0 auto/);
  });

  it("defines each primitive once and never hard-codes its width", () => {
    expect(rulesFor(".pane-col")).toHaveLength(0); // the shared rule is the whole single-column layout
    expect(rulesFor(".pane-split")).toHaveLength(1);
    for (const rule of cssRules().filter((rule) => /\.pane-(col|split)\b/.test(rule.selector))) {
      expect(rule.body).not.toMatch(/max-width:\s*\d/);
    }
  });

  it("gutters both layouts from the same tokens", () => {
    const [body] = rulesFor(".surface #tabbody");
    expect(body).toBeTruthy();
    expect(body.body).toMatch(/padding:\s*var\(--pane-top\) var\(--pane-gutter\) var\(--pane-bottom\)/);
    const [split] = rulesFor(".pane-split");
    expect(split.body).toMatch(/padding:\s*var\(--pane-top\) var\(--pane-gutter\)/);
    expect(split.body).toMatch(/height:\s*100%/);
    expect(split.body).toMatch(/display:\s*flex/);
  });

  it("narrows both layouts through the token, not a per-pane override", () => {
    // :root plus one narrow-viewport override — the gutter never narrows in two
    // places that could drift apart.
    expect(stylesSource.match(/--pane-gutter:/g) || []).toHaveLength(2);
    expect(stylesSource.match(/--pane-top:/g) || []).toHaveLength(2);
    for (const rule of cssRules().filter((rule) => /#tabbody/.test(rule.selector))) {
      expect(rule.body).not.toMatch(/padding:\s*\d+px/);
    }
  });

  it("leaves the bare (terminal / agent) case full-bleed", () => {
    const [bare] = rulesFor(".surface #tabbody.bare");
    expect(bare).toBeTruthy();
    expect(bare.body).toMatch(/padding:\s*0/);
    expect(bare.body).toMatch(/overflow:\s*hidden/);
    expect(bare.body).not.toMatch(/content-max/);
  });
});
