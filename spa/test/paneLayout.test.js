import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

// The pane geometry is split across two sheets now: the primitives and the
// surfaces in styles.css, the three-panel shell (the inbox rail the drawer
// stacks under, the shell tokens) in styles/shell.css. One concatenated source,
// so rules that must agree can be read together — shell first, so its :root
// tokens sit ahead of the narrow-viewport marker tokensAt() slices on.
const stylesSource =
  readFileSync(fileURLToPath(new URL("../src/styles/shell.css", import.meta.url)), "utf8") +
  readFileSync(fileURLToPath(new URL("../src/styles.css", import.meta.url)), "utf8");

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

/** The at-rule prelude a source position sits under, or null at the top level.
 *  Two rules that must agree on a breakpoint are only actually pinned together
 *  when they report the same query. */
function enclosingAtRule(position) {
  let depth = 0;
  let prelude = null;
  for (let i = 0; i < position; i++) {
    const character = strippedSource[i];
    if (character === "@" && depth === 0) {
      prelude = strippedSource.slice(i, strippedSource.indexOf("{", i)).trim().replace(/\s+/g, " ");
    } else if (character === "{") depth += 1;
    else if (character === "}") {
      depth -= 1;
      if (depth === 0) prelude = null;
    }
  }
  return prelude;
}

const NARROW_QUERY = "@media (max-width: 720px)";
const narrowQueryAt = strippedSource.indexOf(NARROW_QUERY);

/** The one width the shell reorganises at: the project sidebar stops being a
 *  column and becomes an overlay, and the two-column panes stack. Both halves
 *  are the same number, which is what makes the transition read as one — the
 *  test below pins that rather than trusting the constant. */
const STACK_QUERY = "@media (max-width: 900px)";
const STACK_WIDTH = 900;

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

/** What the tab shell actually has to hand a pane. Above the stacking width the
 *  inbox rail is a column in flow and costs every pane its width; at or below
 *  it the rail is fixed, so it costs nothing. A model that skips this reads
 *  every desktop pane a rail-width wider than it renders, which is the
 *  difference between a rail that fits and one that does not. */
function shellWidth(viewport) {
  const inFlow = rulesFor("#inbox-rail").find(
    (rule) => enclosingAtRule(rule.at) === null && declaration(rule.body, "width"),
  );
  return viewport > STACK_WIDTH
    ? viewport - pixels(declaration(inFlow.body, "width"), tokensAt(viewport))
    : viewport;
}

/** The two layouts as the shell nests them: .pane-col inside the padded tab
 *  body, .pane-split inside the flush one. */
function layouts(viewport) {
  const tokens = tokensAt(viewport);
  const shared = cssRules().find((rule) => /\.pane-col\b/.test(rule.selector) && /max-width/.test(rule.body));
  const bodyGutter = sidePadding(declaration(rulesFor(".surface #tabbody")[0].body, "padding"), tokens);
  const [split] = rulesFor(".pane-split");
  const frame = shellWidth(viewport);
  return {
    col: contentSpan({
      available: frame - bodyGutter * 2,
      offset: bodyGutter,
      rule: shared.body,
      tokens,
    }),
    split: contentSpan({
      available: frame, // .flush pads nothing; the split states its own gutters
      offset: 0,
      rule: `${shared.body};${split.body}`,
      tokens,
    }),
  };
}

/** Rules that size a primitive itself, rather than something inside one. Only
 *  the last compound of each comma part counts: `.pane-split .pane-list` sizes
 *  the drawer that floats inside the split, and the split's own geometry is no
 *  more its business than a diff row's is. */
const targetsPrimitive = (selector) =>
  selector
    .split(",")
    .some((part) => /\.pane-(col|split)\b/.test(part.trim().split(/\s+/).at(-1)));

describe("tab layout primitives", () => {
  it("declares the content width once, as a :root token", () => {
    expect(stylesSource.match(/--content-max:/g) || []).toHaveLength(1);
    const token = cssRules().find((rule) => rule.selector === ":root" && rule.body.includes("--content-max"));
    expect(token).toBeTruthy();
    expect(token.body).toMatch(/--content-max:\s*1180px/);
  });

  it("sizes both layouts from one shared width rule", () => {
    const widthRules = cssRules().filter(
      (rule) => targetsPrimitive(rule.selector) && /\bmax-width\s*:/.test(rule.body),
    );
    expect(widthRules).toHaveLength(1);
    const [shared] = widthRules;
    expect(shared.selector.split(",").map((part) => part.trim()).sort()).toEqual([".pane-col", ".pane-split"]);
    expect(shared.body).toMatch(/max-width:\s*var\(--content-max\)/);
    expect(shared.body).toMatch(/margin:\s*0 auto/);
    // Neither layout states a width: a block box already fills what it is
    // given, and a stated 100% would fight the gutters the split adds outside
    // its cap.
    for (const rule of cssRules().filter((rule) => targetsPrimitive(rule.selector))) {
      expect(declaration(rule.body, "width")).toBeNull();
    }
  });

  it("defines each primitive once and never hard-codes its width", () => {
    expect(rulesFor(".pane-col")).toHaveLength(0); // the shared rule is the whole single-column layout
    expect(rulesFor(".pane-split")).toHaveLength(1);
    for (const rule of cssRules().filter((rule) => targetsPrimitive(rule.selector))) {
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

  // ---- the two-column layout: Changes and Files ----
  // Their outer box is .pane-split and nothing else. A pane that keeps a
  // display, a height or a padding of its own is a pane the primitive no longer
  // governs, which is how the flush body went edge-to-edge in the first place.
  const TWO_COLUMN_PANES = [".changes2", ".files"];

  it.each(TWO_COLUMN_PANES)("%s leaves its outer box to the primitive", (token) => {
    for (const rule of rulesMentioning(token)) {
      expect(rule.body).not.toMatch(/(^|;)\s*(max-)?width\s*:/);
      expect(rule.body).not.toMatch(/margin[^:]*:[^;]*auto/);
      expect(declaration(rule.body, "display")).toBeNull();
      expect(declaration(rule.body, "height")).toBeNull();
      expect(declaration(rule.body, "padding")).toBeNull();
    }
  });

  it("lets the flush body hand the split no geometry to bypass the cap with", () => {
    const [flush] = rulesFor(".surface #tabbody.flush");
    expect(flush).toBeTruthy();
    // The flush body pads nothing and scrolls nothing — the split states the
    // gutters and the columns own the scroll. If it grew a width or a padding
    // the two-column pane would measure by the body again, not by the token.
    expect(flush.body).toMatch(/padding:\s*0/);
    expect(flush.body).toMatch(/overflow:\s*hidden/);
    for (const rule of cssRules().filter((rule) => /#tabbody\.flush/.test(rule.selector))) {
      expect(rule.body).not.toMatch(/(^|;)\s*(max-)?width\s*:/);
    }
    // .pane-split itself never scrolls: a scrolling split would move the rail
    // and the detail column together instead of each in its own frame.
    const [split] = rulesFor(".pane-split");
    expect(declaration(split.body, "overflow")).toBeNull();
  });

  it("stands the flush body's own pre-pane messages on the same gutter", () => {
    // A surface paints "loading…" and "no checkout carries this branch" into the
    // tab body before either column exists. The flush body pays no gutter, so
    // those messages would sit against the frame — they take the token instead,
    // exactly as the Changes pane's pre-skeleton states do.
    const [message] = cssRules().filter((rule) => rule.selector === ".surface #tabbody.flush > .empty");
    expect(message).toBeTruthy();
    expect(declaration(message.body, "padding")).toBe("var(--pane-top) var(--pane-gutter)");
  });

  it("keeps the Files preview column legible in a pane too narrow for both", () => {
    // The tree is a fixed 300px and the preview takes what is left, so a pane
    // squeezed by a docked inbox and an open agent panel hands the preview
    // nothing — an empty state rendered a few characters wide. The floor is
    // small enough never to bind at a width the two columns actually render at
    // (the tightest is a 901px window, where the preview still clears it), so
    // the tree keeps its full basis everywhere it has the room for it.
    const [preview] = rulesFor(".fpreview");
    expect(preview).toBeTruthy();
    const floor = declaration(preview.body, "min-width");
    expect(floor).toBe("min(200px, 100%)");
    const [tree] = rulesFor(".ftree");
    // …and the tree is what gives ground for it: a column that cannot shrink
    // would overflow the pane instead, and the flush body clips what overflows.
    expect(declaration(tree.body, "flex")).toBe("0 1 300px");
  });

  it("fills its grid column instead of centring over the agent rail", () => {
    // The reading page centres with `margin:0 auto`, and the surface inherits
    // that. But the surface is a grid item beside the agent rail, and a grid
    // item with auto side margins stops stretching: it sizes to its content
    // and a wide diff stack slides under the rail, which then swallows every
    // click on what it covers. The surface must give the margins back.
    const [surface] = rulesFor("main#root.surface");
    expect(surface).toBeTruthy();
    const margin = declaration(surface.body, "margin");
    expect(margin).not.toBeNull();
    expect(margin).not.toMatch(/auto/);
  });

  // Each column scrolls itself. This is what the two-column layout is for, and
  // it is exactly what capping and insetting the pane could break.
  it.each([
    [".crail-host", "overflow-y", "auto"],
    [".cdetail-host", "overflow-y", "auto"],
    [".ftree", "overflow-y", "auto"],
    [".fpbody", "overflow", "auto"],
  ])("keeps %s scrolling internally", (token, property, value) => {
    const scroller = rulesMentioning(token).find((rule) => declaration(rule.body, property));
    expect(scroller).toBeTruthy();
    expect(declaration(scroller.body, property)).toBe(value);
  });

  const RAILS = [".crail-host", ".ftree"];

  /** How wide a rail renders inside an unstacked split: its flex basis, which it
   *  neither grows nor shrinks from, minus whatever a cap takes off it. The cap
   *  can only ever subtract — which is the whole point of measuring it. */
  function railWidth(token, viewport) {
    const tokens = tokensAt(viewport);
    const [rail] = rulesMentioning(token).filter(
      (rule) => enclosingAtRule(rule.at) === null && declaration(rule.body, "flex"),
    );
    const basis = pixels(declaration(rail.body, "flex").split(/\s+/)[2], tokens);
    const gutter = sidePadding(declaration(rulesFor(".pane-split")[0].body, "padding"), tokens);
    const pane = Math.min(shellWidth(viewport) - gutter * 2, pixels("var(--content-max)", tokens));
    const cap = declaration(rail.body, "max-width");
    if (!cap) return basis;
    const against = cap.endsWith("%") ? pane : cap.endsWith("vw") ? viewport : 100;
    return Math.min(basis, /%|vw/.test(cap) ? (Number.parseFloat(cap) / 100) * against : pixels(cap, tokens));
  }

  // The rail's basis is the width its branch button, its dates and its stats
  // were drawn for; a cap that takes room off it hands back a rail that reads
  // worse than the stacked strip one pixel narrower would. The pane is not the
  // window — the sidebar is a 272px column above the stacking width — so the
  // tightest unstacked pane is the one just above it, and that is precisely
  // where a share-of-the-pane cap bites hardest.
  it.each([901, 1000, 1065, 1066, 1400, 2000, 3200])(
    "gives each rail its full basis in a %ipx window",
    (viewport) => {
      for (const token of RAILS) {
        const [rail] = rulesMentioning(token).filter(
          (rule) => enclosingAtRule(rule.at) === null && declaration(rule.body, "flex"),
        );
        expect(railWidth(token, viewport)).toBe(pixels(declaration(rail.body, "flex").split(/\s+/)[2], {}));
      }
    },
  );

  it("caps neither rail while the split is unstacked", () => {
    // Nothing left to cap against: the rail cannot grow past its basis, and
    // below the stacking width it is a full-width strip instead. Any cap here —
    // a viewport share (which measures a box the split no longer spans) or a
    // pane share (which the sidebar drags under the basis) — can only subtract.
    for (const token of RAILS) {
      const caps = rulesMentioning(token)
        .filter((rule) => enclosingAtRule(rule.at) === null)
        .map((rule) => declaration(rule.body, "max-width"))
        .filter(Boolean);
      expect(caps).toEqual([]);
    }
  });

  it("hands the pane the whole frame at the width it stacks at", () => {
    // The inbox rail leaves the flow under the same query the panes stack
    // under. Were they different numbers, one of the two transitions would
    // land in a frame sized for the other.
    const overlay = rulesFor("#inbox-rail").find((rule) => declaration(rule.body, "position") === "fixed");
    expect(overlay).toBeTruthy();
    expect(enclosingAtRule(overlay.at)).toBe(STACK_QUERY);
    expect(shellWidth(STACK_WIDTH)).toBe(STACK_WIDTH);
  });

  // ---- the two-column layout below the stacking width ----
  // Neither pane stacks any more. A phone given two stacked scrollers spends
  // half a small screen on the list it is not reading, so the list floats over
  // the detail instead and a handle pulls it out.
  it("turns the list column into a drawer instead of stacking the pane", () => {
    const stacked = cssRules().filter(
      (rule) => declaration(rule.body, "flex-direction") === "column" && TWO_COLUMN_PANES.includes(rule.selector),
    );
    expect(stacked).toEqual([]);
    const [drawer] = cssRules().filter((rule) => rule.selector === ".pane-split .pane-list");
    expect(drawer).toBeTruthy();
    expect(enclosingAtRule(drawer.at)).toBe(STACK_QUERY);
    expect(declaration(drawer.body, "position")).toBe("absolute");
    expect(declaration(drawer.body, "transform")).toBe("translateX(-100%)");
    expect(declaration(drawer.body, "width")).toBe("var(--pane-drawer)");
    // The split is what the drawer measures and floats against, so it is the
    // containing block — stated once, at every width, where the primitive is.
    expect(declaration(rulesFor(".pane-split")[0].body, "position")).toBe("relative");
  });

  it("drawers both panes from one rule, so neither can drift", () => {
    // The list column belongs to the primitive here, not to Changes and Files
    // separately: two copies of this is how the stacked strip ended up with two
    // maintained-in-parallel halves in the first place.
    for (const token of RAILS) {
      expect(rulesMentioning(token).filter((rule) => enclosingAtRule(rule.at) === STACK_QUERY)).toEqual([]);
    }
  });

  it("rides the handle out with the drawer it opened", () => {
    const open = cssRules().find((rule) => rule.selector === ".pane-split.drawer-open .pane-list");
    expect(open).toBeTruthy();
    expect(declaration(open.body, "transform")).toBe("none");
    const handle = cssRules().find((rule) => rule.selector === ".pane-split.drawer-open .pane-handle");
    expect(declaration(handle.body, "left")).toBe("var(--pane-drawer)");
    // One width for the panel and for how far the handle travels — a handle
    // that stops anywhere but the panel's edge reads as a second control.
    expect(stylesSource.match(/--pane-drawer:/g) || []).toHaveLength(1);
  });

  it("keeps the drawer's chrome out of the wide layout entirely", () => {
    // Both panes carry the handle and the scrim at every width; above the
    // stacking width there is no drawer, so neither may paint.
    const [base] = cssRules().filter((rule) => rule.selector === ".pane-scrim, .pane-handle");
    expect(base).toBeTruthy();
    expect(declaration(base.body, "display")).toBe("none");
    expect(enclosingAtRule(base.at)).toBeNull();
  });

  it("stacks the drawer under the inbox rail it shares the width with", () => {
    // Both overlay at the same width. The inbox rail is the outer surface —
    // a pane's drawer painting over it would trap the user in the pane.
    const zIndex = (token) =>
      Number(
        declaration(
          cssRules().find((rule) => rule.selector.includes(token) && declaration(rule.body, "z-index")).body,
          "z-index",
        ),
      );
    const railFloor = Math.min(zIndex("#inbox-rail"), zIndex("#inbox-scrim"));
    for (const token of [".pane-list", ".pane-scrim", ".pane-handle"]) {
      expect(zIndex(token)).toBeLessThan(railFloor);
    }
  });

  // The regions of the Changes detail column that pay the divider gutter. The
  // Files preview is not among them: its head and body carry their own even
  // padding, so it reads the same at either width.
  const DIVIDER_SIDE = [".cdetail-host", ".gp-toolbar", ".gp-banner"];

  it("gutters the detail column from the tokens, on the divider side only", () => {
    // The split already insets the pane from the frame, so the column that
    // touches the frame pays the gutter once — on its left, against the rail's
    // divider. Repeating it on the right would stop the diffs 44px short of
    // where every one-column tab's content ends.
    for (const token of DIVIDER_SIDE) {
      const [rule] = rulesMentioning(token).filter((rule) => declaration(rule.body, "padding"));
      expect(rule).toBeTruthy();
      const sides = declaration(rule.body, "padding").split(/\s+/);
      expect(sides).toHaveLength(4);
      expect(sides[1]).toBe("0");
      expect(sides[3]).toBe("var(--pane-gutter)");
    }
  });

  it("drops that gutter where the divider is a drawer", () => {
    // Below the stacking width the list column floats over this one instead of
    // sitting beside it, so there is no divider left to clear — and a gutter
    // paid on one side only leaves the diffs off centre in a frame they now
    // have to themselves. One rule for all three regions, under the query the
    // drawer appears at.
    const [evened] = cssRules().filter(
      (rule) => enclosingAtRule(rule.at) === STACK_QUERY && declaration(rule.body, "padding-left") === "0",
    );
    expect(evened).toBeTruthy();
    expect(evened.selector.split(",").map((part) => part.trim()).sort()).toEqual([...DIVIDER_SIDE].sort());
    // Only the divider side goes: the vertical rhythm and the scroll-end room
    // below the last diff are not the gutter's to change.
    expect(declaration(evened.body, "padding")).toBeNull();
  });

  it("stands the Changes pane's pre-skeleton states on the same gutter", () => {
    // The loading and dead-scope states paint before the split exists, so they
    // take the gutter from the token rather than sitting against a frame the
    // split no longer touches.
    const [message] = cssRules().filter((rule) => rule.selector === ".gitpane > .empty");
    expect(message).toBeTruthy();
    expect(declaration(message.body, "padding")).toBe("var(--pane-top) var(--pane-gutter)");
  });

  it("leaves the bare (terminal / agent) case full-bleed", () => {
    const [bare] = rulesFor(".surface #tabbody.bare");
    expect(bare).toBeTruthy();
    expect(bare.body).toMatch(/padding:\s*0/);
    expect(bare.body).toMatch(/overflow:\s*hidden/);
    expect(bare.body).not.toMatch(/content-max/);
  });
});

// The work surfaces' reading columns end with the shared sticky actionbar, and
// a sticky box cannot travel past its parent's content box: scroll-end room
// paid as the column's bottom padding pins the bar that far above the floor and
// leaves a strip of the diff scrolling past underneath it. These rules live in
// styles/surfaces.css — the columns are the surfaces' own, not the primitive's.
describe("the tail of a reading column", () => {
  const surfacesSource = readFileSync(
    fileURLToPath(new URL("../src/styles/surfaces.css", import.meta.url)),
    "utf8",
  ).replace(/\/\*[\s\S]*?\*\//g, "");
  const surfaceRules = () =>
    [...surfacesSource.matchAll(/([^{}@;]+)\{([^{}]*)\}/g)].map((match) => ({
      selector: match[1].trim().replace(/\s+/g, " "),
      body: match[2].trim(),
    }));
  const COLUMNS = [".gitpane .cdetail-host", ".issueview .ivviewer"];

  it("pays its scroll-end room on the tail, not on the column", () => {
    const [column] = surfaceRules().filter(
      (rule) => rule.selector.split(",").map((part) => part.trim()).join(",") === COLUMNS.join(","),
    );
    expect(column).toBeTruthy();
    expect(declaration(column.body, "padding-bottom")).toBe("0");
    const [tail] = surfaceRules().filter((rule) => rule.selector.includes(":last-child:not(.actionbar)"));
    expect(tail).toBeTruthy();
    expect(declaration(tail.body, "margin-bottom")).toBe("var(--pane-bottom)");
    for (const column of COLUMNS) expect(tail.selector).toContain(column);
  });

  it("makes the bar opaque over itself and fades above it", () => {
    // The primitive's gradient fades to nothing across the bar's own box, which
    // puts the hint and whatever is passing under it in the same pixels.
    const [bar] = surfaceRules().filter((rule) => rule.selector.includes("> .actionbar"));
    expect(bar).toBeTruthy();
    expect(declaration(bar.body, "background")).toBe("var(--bg)");
    expect(declaration(bar.body, "box-shadow")).toMatch(/var\(--bg\)$/);
    for (const column of COLUMNS) expect(bar.selector).toContain(column);
  });
});

// The reading page (Settings, Archive, the gate) is not a surface: it is a
// centred column with a cap. On a phone the cap is never the binding constraint
// — the frame is — and a page that sizes to its own content instead of to the
// frame has its right-hand side clipped away by the view column's overflow,
// with no scrollbar to get it back.
describe("the reading page", () => {
  const narrowRule = (selector) =>
    rulesFor(selector).find((rule) => enclosingAtRule(rule.at) === NARROW_QUERY);

  it("never grows past the column it is given, and still centres inside it", () => {
    const [page] = rulesFor("main");
    expect(page).toBeTruthy();
    // Auto margins size a grid item to its content, so the cap has to carry the
    // frame too: min() keeps the centring on a wide screen and the fit on a
    // narrow one.
    expect(declaration(page.body, "max-width")).toBe("min(1180px, 100%)");
    expect(declaration(page.body, "margin")).toBe("0 auto");
  });

  it("wraps the settings rows on a phone rather than holding the page open", () => {
    const row = narrowRule(".projrow");
    expect(row).toBeTruthy();
    expect(declaration(row.body, "flex-wrap")).toBe("wrap");
    // The name takes the first line; the path gives ground instead of pinning
    // the row to its own length.
    expect(declaration(narrowRule(".projrow .pname").body, "min-width")).toBe("0");
    expect(declaration(narrowRule(".projrow .ppath").body, "overflow-wrap")).toBe("anywhere");
    expect(declaration(narrowRule(".addproj").body, "flex-wrap")).toBe("wrap");
  });
});
