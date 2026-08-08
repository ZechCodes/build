import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const stylesSource = readFileSync(
  fileURLToPath(new URL("../src/styles.css", import.meta.url)),
  "utf8",
);

const strippedSource = stylesSource.replace(/\/\*[\s\S]*?\*\//g, "");

/** Every `selector { declarations }` pair in the sheet. Comments are stripped
 *  first and at-rule preludes never match (their inner rules do), so a rule
 *  inside a media query is read exactly like a top-level one — `at` says where
 *  it sits, which is how the narrow-viewport block is told from the base. */
function cssRules() {
  return [...strippedSource.matchAll(/([^{}@;]+)\{([^{}]*)\}/g)].map((match) => ({
    selector: match[1].trim().replace(/\s+/g, " "),
    body: match[2].trim(),
    at: match.index,
  }));
}

const rulesFor = (selector) => cssRules().filter((rule) => rule.selector === selector);

/** The value a rule body settles on for one property, or null. Names are
 *  compared whole, so `width` never reads out of `max-width`, and a repeated
 *  property keeps its last value the way the cascade does. */
function declaration(body, property) {
  let found = null;
  for (const piece of body.split(";")) {
    const colon = piece.indexOf(":");
    if (colon < 0) continue;
    if (piece.slice(0, colon).trim() === property) found = piece.slice(colon + 1).trim();
  }
  return found;
}

const NARROW_QUERY = "@media (max-width: 720px)";
const narrowQueryAt = strippedSource.indexOf(NARROW_QUERY);

/** Custom properties as the cascade leaves them for a viewport: every :root
 *  rule in source order, minus the narrow-viewport block when it does not
 *  apply. */
function tokensAt(viewport) {
  const tokens = {};
  for (const rule of cssRules()) {
    if (rule.selector !== ":root") continue;
    if (viewport > 720 && rule.at > narrowQueryAt) continue;
    for (const [, name, value] of rule.body.matchAll(/(--[\w-]+)\s*:\s*([^;]+)/g)) {
      tokens[name] = value.trim();
    }
  }
  return tokens;
}

const pixels = (value, tokens) => {
  if (value == null) return null;
  const resolved = value.replace(/var\((--[\w-]+)\)/g, (_, name) => tokens[name]);
  return Number.parseFloat(resolved);
};

/** The horizontal half of a padding shorthand. */
const sidePadding = (shorthand, tokens) => {
  if (!shorthand) return 0;
  const parts = shorthand.split(/\s+/);
  return pixels(parts.length === 1 ? parts[0] : parts[1], tokens);
};

/** Where a box's content actually lands: its left edge in the frame and its
 *  width, given the content width its container offers and where that container
 *  starts. This is the part the declaration assertions cannot see — whether a
 *  gutter falls inside or outside the max-width box, and so whether the two
 *  layouts agree once the cap, not the viewport, is the binding constraint. */
function contentSpan({ available, offset, rule, tokens }) {
  const gutter = sidePadding(declaration(rule, "padding"), tokens);
  const cap = pixels(declaration(rule, "max-width"), tokens) ?? Infinity;
  const stated = declaration(rule, "width");
  const borderBox = (declaration(rule, "box-sizing") ?? "border-box") === "border-box";
  let outer;
  if (borderBox) {
    outer = Math.min(available, cap); // width:100% and width:auto both fill
  } else {
    outer = Math.min(stated === "100%" ? available : available - gutter * 2, cap) + gutter * 2;
  }
  // margin:0 auto centres what is left; an over-constrained box hugs the start.
  const left = offset + Math.max(0, (available - outer) / 2);
  return { left: left + gutter, width: outer - gutter * 2 };
}

/** The two layouts as the shell nests them: .pane-col inside the padded tab
 *  body, .pane-split inside the flush one. */
function layouts(viewport) {
  const tokens = tokensAt(viewport);
  const shared = cssRules().find((rule) => /\.pane-col\b/.test(rule.selector) && /max-width/.test(rule.body));
  const bodyGutter = sidePadding(declaration(rulesFor(".surface #tabbody")[0].body, "padding"), tokens);
  const [split] = rulesFor(".pane-split");
  return {
    col: contentSpan({
      available: viewport - bodyGutter * 2,
      offset: bodyGutter,
      rule: shared.body,
      tokens,
    }),
    split: contentSpan({
      available: viewport, // .flush pads nothing; the split states its own gutters
      offset: 0,
      rule: `${shared.body};${split.body}`,
      tokens,
    }),
  };
}

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
    expect(shared.body).toMatch(/max-width:\s*var\(--content-max\)/);
    expect(shared.body).toMatch(/margin:\s*0 auto/);
    // Neither layout states a width: a block box already fills what it is
    // given, and a stated 100% would fight the gutters the split adds outside
    // its cap.
    for (const rule of cssRules().filter((rule) => /\.pane-(col|split)\b/.test(rule.selector))) {
      expect(declaration(rule.body, "width")).toBeNull();
    }
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
    expect(split.body).toMatch(/display:\s*flex/);
    // The split measures its content, so its own gutters land outside the cap
    // the way the tab body's do for the one column — and its height has to shed
    // the top gutter to still fill the frame rather than overflow it.
    expect(declaration(split.body, "box-sizing")).toBe("content-box");
    expect(declaration(split.body, "height")).toBe("calc(100% - var(--pane-top))");
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

  // The declaration assertions above can all pass while the two layouts still
  // render differently: the one-column gutter comes from the tab body, outside
  // the capped box, and the split's comes from itself. Whether those cancel
  // depends on which constraint binds, so check the arithmetic at both regimes —
  // narrow, where the viewport binds, and wide, where the cap does.
  it.each([360, 640, 720, 900, 1224, 1400, 2000, 3200])(
    "puts both layouts' content in the same place at %ipx",
    (viewport) => {
      const { col, split } = layouts(viewport);
      expect(split).toEqual(col);
      expect(col.width).toBeGreaterThan(0);
      expect(col.left).toBeGreaterThanOrEqual(0);
    },
  );

  // Every one-column pane the four tab-shell surfaces paint into the padded tab
  // body, and the elements those panes are built from. None may state a width
  // or a centring margin of its own: .pane-col states both, once, for all of
  // them, which is the only reason a tab switch no longer moves the content's
  // edges. (paneAdoption.test.js checks each of these actually carries it.)
  const ONE_COLUMN_PANES = [
    ".review-thread",
    "#planthread",
    ".cluster-pane",
    ".project-inbox",
    ".issues",
    ".plan",
    ".plan-summary",
    ".plan-feedback",
    ".stagelist",
    ".stage-validation",
    ".fixbar",
    ".stagecomments",
  ];

  /** Whole selector tokens, so `.issues` never reads out of `.issues-empty` and
   *  `.plan` never out of `.plan-summary`. */
  const selectorTokens = (selector) => selector.match(/[.#][-\w]+/g) || [];
  const rulesMentioning = (token) =>
    cssRules().filter((rule) => selectorTokens(rule.selector).includes(token));

  it.each(ONE_COLUMN_PANES)("%s states no width and no centring of its own", (token) => {
    for (const rule of rulesMentioning(token)) {
      expect(rule.body).not.toMatch(/(^|;)\s*(max-)?width\s*:/);
      expect(rule.body).not.toMatch(/margin[^:]*:[^;]*auto/);
    }
  });

  it("keeps no second content width for a one-column pane to fall back to", () => {
    // The three the panes above used to carry. 1180px survives only as the
    // token's value and as the non-surface page's own width.
    for (const orphan of ["920px", "820px", "780px"]) {
      const carriers = cssRules()
        .filter((rule) => new RegExp(`max-width:\\s*${orphan}`).test(rule.body))
        .map((rule) => rule.selector);
      expect(carriers).toEqual(orphan === "780px" ? [".gitcommit"] : []);
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
