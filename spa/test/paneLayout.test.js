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

// The work surfaces' own sheet is not concatenated with those two: its rules
// are the surfaces', not the primitive's, and the tests that read it say so.
const strippedSurfaces = readFileSync(
  fileURLToPath(new URL("../src/styles/surfaces.css", import.meta.url)),
  "utf8",
).replace(/\/\*[\s\S]*?\*\//g, "");

/** Every `selector { declarations }` pair in a sheet. Comments are stripped
 *  first and at-rule preludes never match (their inner rules do), so a rule
 *  inside a media query is read exactly like a top-level one — `at` says where
 *  it sits, which is how the narrow-viewport block is told from the base, and
 *  `sheet` says which source that position is into. */
function rulesIn(source) {
  return [...source.matchAll(/([^{}@;]+)\{([^{}]*)\}/g)].map((match) => ({
    selector: match[1].trim().replace(/\s+/g, " "),
    body: match[2].trim(),
    at: match.index,
    sheet: source,
  }));
}

const cssRules = () => rulesIn(strippedSource);

const rulesFor = (selector) => cssRules().filter((rule) => rule.selector === selector);

/** The rule as it reads before any media query narrows it. */
const baseRule = (selector) => rulesFor(selector).find((rule) => enclosingAtRule(rule.at) === null);

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
function enclosingAtRuleIn(source, position) {
  let depth = 0;
  let prelude = null;
  for (let i = 0; i < position; i++) {
    const character = source[i];
    if (character === "@" && depth === 0) {
      prelude = source.slice(i, source.indexOf("{", i)).trim().replace(/\s+/g, " ");
    } else if (character === "{") depth += 1;
    else if (character === "}") {
      depth -= 1;
      if (depth === 0) prelude = null;
    }
  }
  return prelude;
}

const enclosingAtRule = (position) => enclosingAtRuleIn(strippedSource, position);
/** The same question asked of a rule from either sheet. */
const enclosingAtRuleOf = (rule) => enclosingAtRuleIn(rule.sheet, rule.at);

const NARROW_QUERY = "@media (max-width: 720px)";
const narrowQueryAt = strippedSource.indexOf(NARROW_QUERY);

/** The one width the shell reorganises at: the project sidebar stops being a
 *  column and becomes an overlay, and the two-column panes stack. Both halves
 *  are the same number, which is what makes the transition read as one — the
 *  test below pins that rather than trusting the constant. */
const STACK_QUERY = "@media (max-width: 900px)";

/** The width a phone reads at: the conversation panel stops sitting beside the
 *  work and is laid over it. */
const PHONE_QUERY = "@media (max-width: 760px)";
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

/** The two horizontal sides of a padding shorthand, expanded as CSS expands it.
 *  Both sides, separately: the split pays a gutter on its left and nothing on
 *  its right, so a model that reads one number for "the horizontal padding"
 *  cannot see where either layout actually ends. */
const sidePaddings = (shorthand, tokens) => {
  if (!shorthand) return { left: 0, right: 0 };
  const parts = shorthand.trim().split(/\s+/);
  const [top, right = top, , left = right] = parts;
  return { left: pixels(left, tokens), right: pixels(right, tokens) };
};

/** The horizontal half of a padding shorthand, where a box pays the same on
 *  both sides. */
const sidePadding = (shorthand, tokens) => sidePaddings(shorthand, tokens).right;

/** Where a box's content actually lands: its left edge in the frame and its
 *  width, given the content width its container offers and where that container
 *  starts. This is the part the declaration assertions cannot see — whether a
 *  gutter falls inside or outside the max-width box, and so where each layout
 *  begins and ends once the cap, not the viewport, is the binding constraint. */
function contentSpan({ available, offset, rule, tokens }) {
  const { left: leftGutter, right: rightGutter } = sidePaddings(declaration(rule, "padding"), tokens);
  const gutters = leftGutter + rightGutter;
  const cap = pixels(declaration(rule, "max-width"), tokens) ?? Infinity;
  const stated = declaration(rule, "width");
  const borderBox = (declaration(rule, "box-sizing") ?? "border-box") === "border-box";
  let outer;
  if (borderBox) {
    outer = Math.min(available, cap); // width:100% and width:auto both fill
  } else {
    outer = Math.min(stated === "100%" ? available : available - gutters, cap) + gutters;
  }
  // margin:0 auto centres what is left; an over-constrained box hugs the start.
  const left = offset + Math.max(0, (available - outer) / 2);
  return { left: left + leftGutter, width: outer - gutters };
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
  const bodyGutter = sidePadding(declaration(rulesFor(".surface #tabbody")[0].body, "padding"), tokens);
  const [split] = rulesFor(".pane-split");
  const frame = shellWidth(viewport);
  // No shared width rule any more: a work surface fills the column it is given
  // (flush against the agent rail), so each layout's span is its frame minus
  // its own gutters.
  return {
    col: contentSpan({
      available: frame - bodyGutter * 2,
      offset: bodyGutter,
      rule: "",
      tokens,
    }),
    split: contentSpan({
      available: frame, // .flush pads nothing; the split states its own gutters
      offset: 0,
      rule: split.body,
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
  it("caps and centres neither layout — a work surface fills its column", () => {
    // The reviewer's gap: a centred cap left a dead strip between the diff and
    // the conversation panel on any window wider than the cap. Work surfaces
    // fill what the shell gives them and sit flush against the agent rail;
    // only reading pages (main:not(.surface)) centre inside a cap.
    expect(stylesSource).not.toMatch(/--content-max/);
    for (const rule of cssRules().filter((rule) => targetsPrimitive(rule.selector))) {
      expect(declaration(rule.body, "max-width")).toBeNull();
      expect(declaration(rule.body, "width")).toBeNull();
      expect(declaration(rule.body, "margin")).toBeNull();
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
    expect(split.body).toMatch(/display:\s*flex/);
    // One gutter, on the left, from the same token: the column every surface's
    // first words stand on. The other three sides are the pane's seams with what
    // is around it — the nav bar above, the conversation to the right — and the
    // reviewer's gap was the pane stopping short of both.
    expect(declaration(split.body, "padding")).toBe("0 0 0 var(--pane-gutter)");
    // The split measures its content, so that gutter lands outside the cap the
    // way the tab body's does for the one column. With no top gutter left to
    // shed, the height is simply the frame.
    expect(declaration(split.body, "box-sizing")).toBe("content-box");
    expect(declaration(split.body, "height")).toBe("100%");
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
  // start in different places: the one-column gutter comes from the tab body,
  // outside the capped box, and the split's comes from itself. Whether those
  // cancel depends on which constraint binds, so check the arithmetic at both
  // regimes — narrow, where the viewport binds, and wide, where the cap does.
  it.each([360, 640, 720, 900, 1224, 1400, 2000, 3200])(
    "starts both layouts' content on the same column at %ipx",
    (viewport) => {
      const { col, split } = layouts(viewport);
      expect(split.left).toBe(col.left);
      expect(col.width).toBeGreaterThan(0);
      expect(col.left).toBeGreaterThanOrEqual(0);
    },
  );

  // …and they deliberately END in different places. The one-column layout is
  // read as prose and keeps its right gutter; the split is a tool whose rails,
  // rules and cards run to the seam with the conversation, and only the controls
  // inside it keep their distance (the actionbar, the toolbar's buttons).
  it.each([1224, 1400, 2000, 3200])("runs the split to the seam and insets the one column at %ipx", (viewport) => {
    const { col, split } = layouts(viewport);
    const frame = shellWidth(viewport);
    expect(split.left + split.width).toBe(frame);
    expect(col.left + col.width).toBe(frame - pixels("var(--pane-gutter)", tokensAt(viewport)));
  });

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
    // token's value and as the non-surface page's own width; 780px survives as
    // the width of the box under the diff, which is a line of text to write in
    // rather than a pane.
    for (const orphan of ["920px", "820px", "780px"]) {
      const carriers = cssRules()
        .filter((rule) => new RegExp(`max-width:\\s*${orphan}`).test(rule.body))
        .map((rule) => rule.selector);
      expect(carriers).toEqual(orphan === "780px" ? [".csbox"] : []);
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
    expect(declaration(tree.body, "flex")).toBe("0 1 var(--pane-rail)");
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
    [".crail", "overflow-y", "auto"],
    [".cdetail-host", "overflow-y", "auto"],
    [".ftree-list", "overflow-y", "auto"],
    [".fpbody", "overflow", "auto"],
  ])("keeps %s scrolling internally", (token, property, value) => {
    const scroller = rulesMentioning(token).find((rule) => declaration(rule.body, property));
    expect(scroller).toBeTruthy();
    expect(declaration(scroller.body, property)).toBe(value);
  });

  const RAILS = [".crail-host", ".ftree"];

  /** The one flex shorthand each rail states outside every media query. */
  const railFlexRule = (token) =>
    rulesMentioning(token).filter(
      (rule) => enclosingAtRule(rule.at) === null && declaration(rule.body, "flex"),
    )[0];

  it("gives Changes and Files the same rail, from one token", () => {
    // The reviewer's gap: the Changes rail rendered twice the width of the file
    // tree and squashed the diffs, so the two tabs read as different views. One
    // token, one flex shorthand — a tab switch moves the divider nowhere.
    const shorthands = RAILS.map((token) => declaration(railFlexRule(token).body, "flex"));
    expect(shorthands).toEqual(["0 1 var(--pane-rail)", "0 1 var(--pane-rail)"]);
    expect(stylesSource.match(/--pane-rail:/g) || []).toHaveLength(1);
  });

  it.each(RAILS)("lets %s's content ellipsize inside the rail rather than set its width", (token) => {
    // A flex item's automatic minimum size is its content's min-content width,
    // and a commit subject is one unbreakable nowrap line: without min-width:0
    // the longest subject on the page sets the rail's width and the basis is
    // decoration. The rows are marked up to truncate — this is what lets them.
    expect(declaration(railFlexRule(token).body, "min-width")).toBe("0");
  });

  /** How wide a rail renders inside an unstacked split: its flex basis, which it
   *  never grows past and only gives ground from in a pane too narrow for both
   *  columns, minus whatever a cap takes off it. The cap can only ever
   *  subtract — which is the whole point of measuring it. */
  function railWidth(token, viewport) {
    const tokens = tokensAt(viewport);
    const [rail] = rulesMentioning(token).filter(
      (rule) => enclosingAtRule(rule.at) === null && declaration(rule.body, "flex"),
    );
    const basis = pixels(declaration(rail.body, "flex").split(/\s+/)[2], tokens);
    const gutter = sidePadding(declaration(rulesFor(".pane-split")[0].body, "padding"), tokens);
    const pane = shellWidth(viewport) - gutter * 2;
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
        expect(railWidth(token, viewport)).toBe(
          pixels(declaration(rail.body, "flex").split(/\s+/)[2], tokensAt(viewport)),
        );
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
    const overlay = rulesFor("#inbox-rail").find((rule) => declaration(rule.body, "position") === "absolute");
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
    // The list column is stated at both widths — a rail that meets the frame
    // above the stacking width, a drawer at or below it — so this is the one
    // under the query, not merely the first.
    const [drawer] = cssRules().filter(
      (rule) => rule.selector === ".pane-split .pane-list" && enclosingAtRule(rule.at) === STACK_QUERY,
    );
    expect(drawer).toBeTruthy();
    expect(declaration(drawer.body, "position")).toBe("absolute");
    expect(declaration(drawer.body, "transform")).toBe("translateX(-100%)");
    expect(declaration(drawer.body, "width")).toBe("var(--pane-drawer)");
    // The split is what the drawer measures and floats against, so it is the
    // containing block — stated once, at every width, where the primitive is.
    expect(declaration(rulesFor(".pane-split")[0].body, "position")).toBe("relative");
  });

  it("gives every branch navigation control the full narrow rail", () => {
    const [cell] = cssRules().filter((rule) => rule.selector === ".branch-tab");
    expect(cell).toBeTruthy();
    expect(declaration(cell.body, "width")).toBe("100%");
    expect(declaration(cell.body, "justify-content")).toBe("center");
    expect(declaration(cell.body, "flex-direction")).toBe("column");
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
  // The banner is not a region of its own any more: it stands in the toolbar,
  // taking the width of the verbs it replaces.
  const DIVIDER_SIDE = [".cdetail-host", ".gp-toolbar"];

  it("gutters every region of the detail column on the divider side", () => {
    // The column touches the frame on its left, so each of its regions pays the
    // gutter there — against the rail's divider, on the same text column the
    // rail's own rows stand on.
    for (const token of DIVIDER_SIDE) {
      const [rule] = rulesMentioning(token).filter((rule) => declaration(rule.body, "padding"));
      expect(rule).toBeTruthy();
      const sides = declaration(rule.body, "padding").split(/\s+/);
      expect(sides).toHaveLength(4);
      expect(sides[3]).toBe("var(--pane-gutter)");
    }
  });

  it("pays the seam gutter on the reading columns, and their boxes pay it once", () => {
    // The reviewer's screenshot: diffs ran to the agent rail's border with no
    // room on their right. The reading column pays the gutter on the seam side
    // now — and the boxes inside it that used to state that inset themselves
    // (the sticky bar, the comment tray) no longer do, or they would sit a
    // double gutter short of the seam.
    const rightOf = (token) => {
      const [rule] = rulesMentioning(token).filter((rule) => declaration(rule.body, "padding"));
      return declaration(rule.body, "padding").split(/\s+/)[1];
    };
    expect(rightOf(".cdetail-host")).toBe("var(--pane-gutter)");
    for (const controls of [".gp-toolbar"]) {
      expect(rightOf(controls)).toBe("var(--pane-gutter)");
    }
    const surfaceRules = rulesIn(strippedSurfaces);
    const viewer = surfaceRules.find((rule) => rule.selector.trim() === ".ivviewer");
    expect(declaration(viewer.body, "padding").split(/\s+/)[1]).toBe("var(--pane-gutter)");
    const bar = surfaceRules.find((rule) => rule.selector.includes("> .actionbar"));
    expect(declaration(bar.body, "padding-right")).toBeNull();
    const tray = surfaceRules.find((rule) => rule.selector.trim() === ".csfeedback");
    expect(declaration(tray.body, "padding-right")).toBeNull();
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

describe("diff file headers", () => {
  it("stick flush to padded reading columns until their file scrolls away", () => {
    const [file] = rulesFor(".file");
    const [header] = rulesFor(".file .fhead");
    const [expandOverlay] = rulesFor(".file.capped .diff-expand");

    expect(declaration(file.body, "overflow")).toBe("clip");
    expect(declaration(header.body, "position")).toBe("sticky");
    expect(declaration(header.body, "top")).toBe("var(--diff-sticky-top, 0)");
    expect(declaration(header.body, "background")).toBe("var(--panel)");
    expect(Number(declaration(header.body, "z-index"))).toBeGreaterThan(
      Number(declaration(expandOverlay.body, "z-index")),
    );

    const [singleColumn] = rulesFor(".surface #tabbody:not(.bare):not(.flush)");
    const [changesColumn] = rulesFor(".cdetail-host");
    const [issueColumn] = rulesIn(strippedSurfaces).filter((rule) => rule.selector === ".ivviewer");
    for (const scroller of [singleColumn, changesColumn, issueColumn]) {
      expect(declaration(scroller.body, "--diff-sticky-top")).toBe("calc(0px - var(--pane-top))");
    }
  });
});

// The work surfaces' reading columns end with the shared sticky actionbar, and
// a sticky box cannot travel past its parent's content box: scroll-end room
// paid as the column's bottom padding pins the bar that far above the floor and
// leaves a strip of the diff scrolling past underneath it. These rules live in
// styles/surfaces.css — the columns are the surfaces' own, not the primitive's.
describe("the tail of a reading column", () => {
  const surfaceRules = () => rulesIn(strippedSurfaces);
  const COLUMNS = [".gitpane .cdetail-host", ".issueview .ivviewer"];

  it("pays its scroll-end room on the tail, not on the column", () => {
    const [column] = surfaceRules().filter(
      (rule) => rule.selector.split(",").map((part) => part.trim()).join(",") === COLUMNS.join(","),
    );
    expect(column).toBeTruthy();
    expect(declaration(column.body, "padding-bottom")).toBe("0");
    // The issue viewer ends on whatever it was written with; the Changes column
    // ends on the keyed stack, which is the one block always at its foot.
    const [tail] = surfaceRules().filter((rule) => rule.selector.includes(":last-child:not(.actionbar)"));
    expect(tail).toBeTruthy();
    expect(tail.selector).toContain(".issueview .ivviewer");
    expect(declaration(tail.body, "margin-bottom")).toBe("var(--pane-bottom)");
    const [stack] = surfaceRules().filter((rule) => rule.selector === ".gitpane .dstack");
    expect(stack).toBeTruthy();
    expect(declaration(stack.body, "padding-bottom")).toBe("var(--pane-bottom)");
  });

  it("makes the bar opaque over itself and fades above it", () => {
    // The primitive's gradient fades to nothing across the bar's own box, which
    // puts the hint and whatever is passing under it in the same pixels.
    const [bar] = surfaceRules().filter((rule) => rule.selector.includes("> .actionbar"));
    expect(bar).toBeTruthy();
    expect(declaration(bar.body, "background")).toBe("var(--bg)");
    expect(declaration(bar.body, "box-shadow")).toMatch(/var\(--bg\)$/);
    // The changeset's bar lives in the tray it is painted with, and the tray is
    // no box of its own — so the rule names the tray, not the whole column.
    expect(bar.selector).toContain(".gitpane .cstray > .actionbar");
    expect(bar.selector).toContain(".issueview .ivviewer > .actionbar");
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

// ---- the column every surface's first text stands on -------------------------
// The toolbar names where you are standing, and its project name is the
// leftmost text in the view column. Everything under it — the tab labels, the
// panes those tabs open — has to start on that same line, or the surface reads
// as inset from the bar that names it. --pane-gutter IS that column: each row
// states the inset it carries itself and pays the difference outside it, so
// there is one number to move and nothing to keep in step by hand.

/** Top-level pieces of an expression, split on `separator` outside parens. */
function topLevel(expression, separator) {
  const pieces = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < expression.length; i += 1) {
    const character = expression[i];
    if (character === "(") depth += 1;
    else if (character === ")") depth -= 1;
    else if (depth === 0 && expression.startsWith(separator, i)) {
      pieces.push(expression.slice(start, i));
      i += separator.length - 1;
      start = i + 1;
    }
  }
  pieces.push(expression.slice(start));
  return pieces;
}

/** Whether the first `(` in an expression is closed by its last character —
 *  which is what tells `max(a, b)` from `max(a, b) + 2px`. */
function closesAtEnd(expression) {
  let depth = 0;
  for (let i = expression.indexOf("("); i >= 0 && i < expression.length; i += 1) {
    if (expression[i] === "(") depth += 1;
    else if (expression[i] === ")") depth -= 1;
    if (depth === 0) return i === expression.length - 1;
  }
  return false;
}

/** A length as the cascade resolves it: custom properties substituted (through
 *  as many levels as they nest), then the small arithmetic the insets are
 *  written with — calc(), max()/min(), and the +, -, * between pixel terms.
 *  CSS demands spaces around + and -, which is what makes the split safe. */
function length(value, tokens) {
  if (!value) return 0;
  let text = value;
  for (let pass = 0; pass < 8 && text.includes("var("); pass += 1) {
    text = text.replace(/var\((--[\w-]+)\)/g, (_, name) => tokens[name] ?? "0");
  }
  const resolve = (expression) => {
    const trimmed = expression.trim();
    const call = /^(calc|max|min)\(([\s\S]*)\)$/.exec(trimmed);
    if (call && closesAtEnd(trimmed)) {
      const parts = topLevel(call[2], ",").map(resolve);
      if (call[1] === "max") return Math.max(...parts);
      if (call[1] === "min") return Math.min(...parts);
      return parts[0];
    }
    const added = topLevel(trimmed, " + ");
    if (added.length > 1) return added.map(resolve).reduce((sum, term) => sum + term);
    const subtracted = topLevel(trimmed, " - ");
    if (subtracted.length > 1) return subtracted.map(resolve).reduce((rest, term) => rest - term);
    const multiplied = topLevel(trimmed, "*");
    if (multiplied.length > 1) return multiplied.map(resolve).reduce((product, term) => product * term);
    return Number.parseFloat(trimmed) || 0;
  };
  return resolve(text);
}

/** One side of a box shorthand, expanded the way CSS expands it. Values never
 *  carry a bare space of their own — every inset here is a token or a calc — so
 *  whitespace is the separator. */
const shorthandSide = (shorthand, side) => {
  const parts = shorthand.trim().split(/\s+/);
  const [top, right = top, bottom = top, left = right] = parts;
  return [top, right, bottom, left][side];
};

/** Both sheets a surface's left edge is written across: the shell's primitives
 *  and the panes in the shared source above, the work surfaces' own rows (the
 *  issue rail) in styles/surfaces.css. */
const everyRule = () => [...cssRules(), ...rulesIn(strippedSurfaces)];

/** The rules for a selector that apply at a viewport, in cascade order. */
const applicableRules = (selector, viewport) =>
  everyRule().filter((rule) => {
    if (rule.selector !== selector) return false;
    const at = enclosingAtRuleOf(rule);
    return at === null || (viewport <= 720 && at === NARROW_QUERY);
  });

/** How far one box moves its content right: the left padding it pays plus the
 *  margin it bleeds out with (negative for a column that meets the frame). */
function leftInset(selector, viewport) {
  const rules = applicableRules(selector, viewport);
  if (!rules.length) throw new Error(`no rule states ${selector}`);
  const tokens = tokensAt(viewport);
  let inset = 0;
  for (const rule of rules) {
    for (const [property, side] of [
      ["padding", 3],
      ["margin", 3],
    ]) {
      const shorthand = declaration(rule.body, property);
      const explicit = declaration(rule.body, `${property}-left`);
      if (shorthand) inset += length(shorthandSide(shorthand, side), tokens);
      if (explicit) inset += length(explicit, tokens);
    }
  }
  return inset;
}

/** Where a chain of nested boxes puts its text. */
const textColumn = (chain, viewport) =>
  chain.reduce((left, selector) => left + leftInset(selector, viewport), 0);

describe("the surface's text column", () => {
  const FLUSH_BODY = ".surface #tabbody.flush";
  const LIST_COLUMN = ".pane-split .pane-list";

  // Each row of the surface, as the boxes its text sits inside. The tab body is
  // where every pane starts: padded for the one column, flush for the split —
  // and in the flush one the list column bleeds back out to the frame, so the
  // row itself is what pays the distance. Every row of all three rails is here:
  // one that is not is one that can drift.
  const inRail = (row) => [FLUSH_BODY, ".pane-split", LIST_COLUMN, row];
  const CHAIN = {
    "the toolbar's project name": [".toolbar", ".tb-sel"],
    "a one-column pane": [".surface #tabbody"],
    "a section heading in the Changes rail": inRail(".crail .rhead"),
    "a changeset in the Changes rail": inRail(".crail .rrow"),
    "a commit in the Changes rail": inRail(".crow"),
    "the Changes rail's empty section": inRail(".crail .empty"),
    "the Changes rail's show-more row": inRail(".gitmore"),
    "the Files tree's crumb": inRail(".fcrumb"),
    "a file in the Files tree": inRail(".frow"),
    "the issue rail's head": inRail(".ivhead"),
    "a stage in the issue rail": inRail(".ivstages .stagerow"),
    "the issue rail with no stages": inRail(".ivstages .empty"),
    // The assignment's fields are not a row of the rail at all — they are the
    // overlay that row opens, and an overlay stands on the frame, not the
    // column.
    "the issue rail's assignment control": inRail(".ivassign-head"),
    "the issue rail's lineage": inRail(".ivlineage"),
  };

  it.each(Object.keys(CHAIN))("stands %s on the gutter, and nowhere else", (row) => {
    const viewport = 1400;
    expect(textColumn(CHAIN[row], viewport)).toBe(pixels("var(--pane-gutter)", tokensAt(viewport)));
  });

  // The phone narrows the gutter through the token; the chains that are not the
  // drawer's follow it there too. (Below the stacking width the list column is
  // an overlay positioned against the split's border box, so its rows' own
  // inset is the whole distance — the test below pins that instead.)
  it.each(["the toolbar's project name", "a one-column pane"])(
    "keeps %s on the gutter where the phone narrows it",
    (row) => {
      expect(textColumn(CHAIN[row], 390)).toBe(pixels("var(--pane-gutter)", tokensAt(390)));
    },
  );

  it("pays each bar's gutter around the inset its own control carries", () => {
    // A bar cannot simply pay the gutter: its buttons have padding of their own,
    // and a pill that pays 13px inside a bar that already paid the gutter puts
    // its label 13px past the column. Each states the difference, from the same
    // two tokens the control itself is padded from — one number to move.
    const [toolbar] = rulesFor(".toolbar");
    expect(shorthandSide(declaration(toolbar.body, "padding"), 3)).toBe("var(--toolbar-gutter)");
    expect(declaration(rulesFor(".tb-sel")[0].body, "padding")).toMatch(/var\(--tbsel-inset\)/);
    for (const bar of rulesFor(".railtabs")) {
      expect(shorthandSide(declaration(bar.body, "padding"), 3)).toBe("var(--tabbar-gutter)");
    }
    expect(declaration(rulesFor(".tabs .t")[0].body, "padding")).toMatch(/var\(--tab-inset\)/);
    // Both derived gutters are stated once, against the one gutter token.
    for (const derived of ["--toolbar-gutter", "--tabbar-gutter"]) {
      expect(stylesSource.match(new RegExp(`${derived}:`, "g")) || []).toHaveLength(1);
      const [token] = cssRules().filter((rule) => rule.selector === ":root" && rule.body.includes(`${derived}:`));
      expect(token.body).toMatch(new RegExp(`${derived}:[^;]*var\\(--pane-gutter\\)`));
    }
    // …and a pill can never inset past the column it stands on, which is what
    // would drive the bar's own gutter negative on a phone.
    for (const viewport of [1400, 390]) {
      const tokens = tokensAt(viewport);
      expect(length("var(--tab-inset)", tokens)).toBeLessThanOrEqual(length("var(--pane-gutter)", tokens));
      expect(length("var(--tabbar-gutter)", tokens)).toBeGreaterThanOrEqual(0);
    }
  });

  it("lets the flush pane's list column meet the frame", () => {
    // The commit rail, the file tree and the issue's stage list are rails, not
    // cards: their fills and their divider run to the frame's edge the way the
    // inbox rail's do, so the column bleeds back through the split's gutter and
    // its rows carry that gutter inside them instead.
    const [bleed] = applicableRules(LIST_COLUMN, 1400);
    expect(bleed).toBeTruthy();
    expect(declaration(bleed.body, "margin-left")).toBe("calc(var(--pane-gutter) * -1)");
    expect(enclosingAtRule(bleed.at)).toBeNull();
    // One rule for all three, like the drawer below it — a per-pane copy is how
    // the rails drifted apart before.
    for (const token of [".crail-host", ".ftree", ".ivstages"]) {
      const railRules = cssRules().filter((rule) => (rule.selector.match(/[.#][-\w]+/g) || []).includes(token));
      expect(railRules.filter((rule) => declaration(rule.body, "margin-left"))).toEqual([]);
    }
  });

  it("gives the bleed back where the column is a drawer", () => {
    // An overlay is positioned against the split's border box, so it already
    // starts at the frame; the negative margin would drag it off-screen.
    const [drawer] = cssRules().filter(
      (rule) => rule.selector === LIST_COLUMN && enclosingAtRule(rule.at) === STACK_QUERY,
    );
    expect(drawer).toBeTruthy();
    expect(declaration(drawer.body, "left")).toBe("0");
    expect(declaration(drawer.body, "margin-left")).toBe("0");
  });
});

// ---- the seam between the view column and the agent rail ---------------------
// The reviewer's screenshot: the work area's right edge and the conversation
// panel were fenced apart — two hairlines with a strip of page between them.
// They are two halves of one frame and have to touch across ONE border. The
// wrapper (#agent-rail) drew the seam while the panel drew a second rule on its
// other side, and where those two met — the phone's overlay, whose right edge
// lands on the strip — the divide was painted twice on one pixel. Each divide
// is stated once now, by the surface that begins at it.
describe("the view column's seam with the agent rail", () => {
  const SIDE_BORDERS = ["border", "border-left", "border-right"];

  /** The rule for a selector outside every media query — the one that holds at
   *  any width, which is where a seam has to be stated. */

  /** Every vertical border a rule draws, as `property:value`. */
  const sideBorders = (rule) =>
    SIDE_BORDERS.map((property) => [property, declaration(rule.body, property)])
      .filter(([, value]) => value)
      .map(([property, value]) => `${property}:${value}`);

  it("puts branch navigation, the view, and the agent rail side by side with nothing between them", () => {
    const body = baseRule("#view-body");
    expect(body).toBeTruthy();
    expect(declaration(body.body, "grid-template-columns")).toBe("auto minmax(0, 1fr) auto");
    // A gutter between the tracks would be a strip of page the seam's border
    // could not cover — the columns meet, and the border is the whole divide.
    for (const rule of rulesFor("#view-body")) {
      for (const spacing of ["gap", "column-gap", "grid-column-gap", "grid-gap"]) {
        expect(declaration(rule.body, spacing)).toBeNull();
      }
    }
  });

  it("leaves the view column's trailing edge bare", () => {
    // The surface draws no rule and pays no gutter where the rail begins: the
    // frame's own inset (a phone's rounded corner) is all it states on that
    // side, and the rail's border is the divide.
    const surface = baseRule("main#root.surface");
    expect(surface).toBeTruthy();
    expect(declaration(surface.body, "margin")).toBe("0");
    expect(sideBorders(surface)).toEqual([]);
    expect(declaration(surface.body, "padding")).toBe(
      "0 env(safe-area-inset-right, 0px) env(safe-area-inset-bottom, 0px) 0",
    );
  });

  it("gives each divide one border, stated by the surface that begins at it", () => {
    // #agent-rail is the wrapper, not a surface. A border on it PLUS a border
    // on the panel inside it is the doubled seam: two rules for one divide,
    // which drift apart the moment either surface moves.
    expect(sideBorders(baseRule("#agent-rail"))).toEqual([]);
    expect(sideBorders(baseRule(".rail-panel"))).toEqual(["border-left:1px solid var(--line)"]);
    expect(sideBorders(baseRule(".rail-strip"))).toEqual(["border-left:1px solid var(--line)"]);
    // Two surfaces, two leading edges, two borders — and no rule anywhere in
    // the rail draws a trailing one for another surface's edge to land on.
    const trailing = ["#agent-rail", ".rail-panel", ".rail-strip"].flatMap((selector) =>
      rulesFor(selector).map((rule) => declaration(rule.body, "border-right")).filter(Boolean),
    );
    expect(trailing).toEqual([]);
  });

  it("keeps the panel flush against that border", () => {
    // Nothing between the border and the conversation: the panel's rows carry
    // their own insets, and a margin here would reopen the strip of page the
    // seam is supposed to have closed.
    const panel = baseRule(".rail-panel");
    for (const property of ["margin", "margin-left", "margin-right", "padding", "padding-left"]) {
      expect(declaration(panel.body, property)).toBeNull();
    }
  });

  it("holds the seam where the phone lays the panel over the view", () => {
    const overlay = rulesFor(".rail-panel").find((rule) => enclosingAtRule(rule.at) === PHONE_QUERY);
    expect(overlay).toBeTruthy();
    // The overlay's leading edge is the same border the docked panel states, and
    // its trailing edge stops ON the strip's — so the panel covers the work
    // beside it without painting a second line over the strip's own.
    expect(sideBorders(overlay)).toEqual([]);
    expect(declaration(overlay.body, "right")).toBe("var(--agent-strip)");
  });

  it("stops the full console on the same line, so the seam runs unbroken", () => {
    // The console at full is an overlay over the view column. It clears the
    // strip by the strip's own width, which is where the strip's border is —
    // one pixel further and the overlay would paint out the divide it stops at.
    const full = cssRules().find((rule) => rule.selector === '#console-region[data-size="full"]');
    expect(full).toBeTruthy();
    expect(declaration(full.body, "right")).toBe("var(--agent-strip)");
    for (const size of ["collapsed", "half", "full"]) {
      const rule = cssRules().find((r) => r.selector === `#console-region[data-size="${size}"]`);
      if (rule) expect(sideBorders(rule)).toEqual([]);
    }
  });

  it("measures the strip, its border and everything that stops at it from one token", () => {
    // The strip's border is INSIDE its stated width (the sheet is border-box),
    // so --agent-strip is the one number the console overlay, the phone's panel
    // and the strip itself all land on.
    expect(stylesSource.match(/--agent-strip:/g) || []).toHaveLength(1);
    expect(declaration(baseRule(".rail-strip").body, "width")).toBe("var(--agent-strip)");
    expect(cssRules().find((rule) => rule.selector === "*" && declaration(rule.body, "box-sizing"))).toBeTruthy();
  });
});

// ---- the conversation panel's two shapes ------------------------------------
// The pin in the panel's head says which of two things the conversation is.
// Pinned it is the column the seam tests above measure — half the frame, beside
// the work, costing the work its width. Unpinned it is a card ON the strip,
// pointing with a notch at the bubble it was opened from, over a scrim that is
// also the way out of it: the shape the away inbox wears at the other edge of
// the frame, turned round to face the strip.
describe("the conversation panel unpinned", () => {
  const POPOVER = "#agent-rail.rail-popover .rail-panel";

  it("floats as a card on the strip's edge, over a scrim", () => {
    const card = baseRule(POPOVER);
    expect(card).toBeTruthy();
    expect(declaration(card.body, "position")).toBe("absolute");
    // It stops on the strip's own edge, from the same token everything that
    // stops there reads.
    expect(declaration(card.body, "right")).toBe("var(--agent-strip)");
    // The rail is the card's frame, so the card cannot land anywhere else.
    const railBox = rulesFor("#agent-rail").find((rule) => declaration(rule.body, "position"));
    expect(declaration(railBox.body, "position")).toBe("relative");
    expect(enclosingAtRule(railBox.at)).toBeNull();

    const scrim = baseRule(".rail-scrim");
    expect(declaration(scrim.body, "position")).toBe("fixed");
    expect(declaration(scrim.body, "inset")).toBe("0");
    expect(declaration(scrim.body, "background")).toBe("var(--scrim)");
    // …and it lies under the card it dismisses.
    expect(Number(declaration(scrim.body, "z-index")))
      .toBeLessThan(Number(declaration(card.body, "z-index")));
  });

  it("holds the strip over the scrim, so the next bubble re-anchors the card", () => {
    // The scrim is what dismisses the card, and it lies over everything under
    // it — including the row of bubbles the card is anchored to. Pressing
    // another agent has to reach that agent, not the way out, so the strip
    // rides above the scrim while the card is open.
    const lifted = baseRule("#agent-rail.rail-popover .rail-strip");
    expect(lifted).toBeTruthy();
    expect(Number(declaration(lifted.body, "z-index")))
      .toBeGreaterThan(Number(declaration(baseRule(".rail-scrim").body, "z-index")));
    // …which it can only do from a position of its own.
    expect(declaration(baseRule(".rail-strip").body, "position")).toBe("relative");
  });

  it("points its notch at the bubble it was opened from", () => {
    const notch = baseRule(`${POPOVER}::before`);
    expect(notch).toBeTruthy();
    // core/agentRail.js measures the open bubble and writes the offset; the
    // clamp keeps the notch on the card for a bubble at either end of a long
    // strip.
    expect(declaration(notch.body, "top")).toBe("clamp(14px, var(--rail-anchor, 50%), calc(100% - 14px))");
    // The same notch the away inbox wears, so the two popovers read as one
    // vocabulary rather than two.
    const inbox = cssRules().find((rule) => rule.selector.includes("#inbox-rail::before"));
    for (const property of ["transform", "width", "height", "background"]) {
      expect([property, declaration(notch.body, property)])
        .toEqual([property, declaration(inbox.body, property)]);
    }
  });
});

describe("the collapsed toolbar's clearance", () => {
  // The clearance accounts for the toggle's left offset, width, and the gap
  // before toolbar content without reserving room for removed branding.
  it("derives a compact clearance from the toggle", () => {
    expect(strippedSource).toMatch(/--inbox-toggle:28px/);
    expect(strippedSource).toMatch(/--inbox-open-clear:calc\(48px \+ env\(safe-area-inset-left, 0px\)\)/);
    expect(strippedSource).toMatch(/\.toolbar \{[^}]*transition:padding-left 240ms cubic-bezier\(\.2,\.8,\.2,1\)/);
    // Both toggles wear the width the clearance is derived from…
    const toggles = cssRules().find((rule) => rule.selector.includes("#inbox-open") && rule.selector.includes("#inbox-collapse"));
    expect(toggles).toBeTruthy();
    expect(declaration(toggles.body, "width")).toBe("var(--inbox-toggle)");
    // …and the head's gap is the 8px the calc pays.
    expect(declaration(rulesFor(".inbox-head")[0].body, "gap")).toBe("8px");
  });
});

describe("the rail status line", () => {
  it("anchors the git group to the row's right, so the timer widens into space", () => {
    const git = rulesFor(".rail-status-git")[0];
    expect(git).toBeTruthy();
    expect(declaration(git.body, "margin-left")).toBe("auto");
  });
});

describe("the rail surface menu", () => {
  // On phones the shared split-menu rule left-aligns menus with their trigger.
  // This trigger sits at the viewport's right edge, so its menu must grow left.
  it("anchors its right edge to the header action", () => {
    const rule = rulesFor(".rail-surface-menu .splitmenu")[0];
    expect(rule).toBeTruthy();
    expect(declaration(rule.body, "right")).toBe("0");
    expect(declaration(rule.body, "left")).toBe("auto");
    expect(declaration(rule.body, "max-width")).toBe("calc(100vw - 16px)");
  });
});

// ---- the row of heads across the top of a work surface ----------------------
// The reviewer's screenshot: at desktop width the ref picker (left of the
// pane), the git action bar (middle) and the conversation panel's head (right)
// sat side by side at three different heights, so their bottom borders stepped
// down across the frame. They are one row of chrome and have to read as one
// line — which means one number, stated once, that all three stop on.
describe("the row of heads across a work surface", () => {
  const SURFACE_HEADS = [".workspace-refbar", ".gp-toolbar", ".rail-head"];

  /** The same question asked of any of the three sheets: the three bars live in
   *  three files and are only actually pinned together when they are read
   *  together. */
  const headRule = (selector) =>
    [...cssRules(), ...rulesIn(strippedSurfaces)].find(
      (rule) => rule.selector === selector && enclosingAtRuleOf(rule) === null,
    );

  it("states the shared height once, beside the row above it", () => {
    // One token across all three sheets — a second definition is a second
    // number, and two numbers are what put the borders on different lines.
    expect((stylesSource + strippedSurfaces).match(/--surface-head:/g) || []).toHaveLength(1);
    expect(tokensAt(1400)["--surface-head"]).toBe("46px");
    // It is the sibling of the view column's own top row, one row down.
    expect(tokensAt(1400)["--toolbar-h"]).toBe("38px");
  });

  it("gives all three bars that one height and centres what they carry", () => {
    for (const selector of SURFACE_HEADS) {
      const rule = headRule(selector);
      expect([selector, !!rule]).toEqual([selector, true]);
      expect([selector, declaration(rule.body, "min-height")]).toEqual([selector, "var(--surface-head)"]);
      // A bar taller than its contents has to say where they sit in it, or the
      // three sets of controls line up at three different heights inside one
      // shared box.
      expect([selector, declaration(rule.body, "display")]).toEqual([selector, "flex"]);
      expect([selector, declaration(rule.body, "align-items")]).toEqual([selector, "center"]);
      // …and one border under the row, so the shared height is a shared line.
      expect([selector, declaration(rule.body, "border-bottom")]).toEqual([selector, "1px solid var(--line)"]);
    }
  });

  it("lets each bar's own contents take its width", () => {
    // The bars are flex rows now; the one thing on each has to fill it rather
    // than shrink to its text.
    expect(declaration(headRule(".workspace-refpicker").body, "flex")).toBe("1 1 auto");
    expect(declaration(headRule(".gittoolbar").body, "flex")).toBe("1 1 auto");
  });
});

describe("the git toolbar's menus", () => {
  // The reviewer's screenshot: the Push menu opened upward from the git bar and
  // the navigation bar above cut it off. The bar sits at the top of its pane,
  // so its menus have all the room below and none above.
  it("open downward, out from under the navigation bar", () => {
    const rule = rulesFor(".gp-toolbar .splitmenu")[0];
    expect(rule).toBeTruthy();
    expect(declaration(rule.body, "top")).toBe("calc(100% + 6px)");
    expect(declaration(rule.body, "bottom")).toBe("auto");
  });
});

describe("the inbox row's actions", () => {
  // The reviewer's screenshot: Done + ⋯ sat in flow and squeezed the facts
  // line into a wrap ("74 files · +1633" / "−8871"). The actions overlay the
  // row's top-right instead, so the row's full width belongs to its words.
  it("overlay the row instead of costing it width", () => {
    const actions = rulesFor(".inbox-actions").find(
      (rule) => declaration(rule.body, "position") === "absolute",
    );
    expect(actions).toBeTruthy();
    expect(declaration(actions.body, "opacity")).toBe("0");
    expect(declaration(actions.body, "pointer-events")).toBe("none");
    const anchored = rulesFor(".inbox-entry").some(
      (rule) => declaration(rule.body, "position") === "relative",
    );
    expect(anchored).toBe(true);
  });

  it("reveals for pointer, keyboard, selection, and the open menu alike", () => {
    const reveal = cssRules().find(
      (rule) =>
        rule.selector.includes(".inbox-entry:hover .inbox-actions") &&
        declaration(rule.body, "opacity") === "1" &&
        declaration(rule.body, "pointer-events") === "auto",
    );
    expect(reveal).toBeTruthy();
    expect(reveal.selector).toContain(".inbox-entry:focus-within .inbox-actions");
    expect(reveal.selector).toContain(".inbox-entry.active .inbox-actions");
    expect(reveal.selector).toContain(".inbox-actions:has(.inbox-menu:not([hidden]))");
  });

  it("stays reachable where hover does not exist", () => {
    const touch = cssRules().find(
      (rule) =>
        enclosingAtRule(rule.at) === "@media (hover:none)" &&
        rule.selector.includes(".inbox-entry.active .inbox-actions") &&
        declaration(rule.body, "opacity") === "1",
    );
    expect(touch).toBeTruthy();
    expect(touch.selector).toContain(".inbox-entry .inbox-actions:has(.inbox-workspace-done)");
  });
});

describe("the creation sheet", () => {
  // Add project with two sources added was taller than a 1440x950 frame, and
  // neither the sheet nor the scrim scrolled, so Create sat below the fold with
  // no way to reach it. The phone block had bounded the sheet all along; the
  // bound belongs at every width. The scrim leaves the same room above and
  // below, and the sheet takes what is left and scrolls inside it.
  it("is bounded by the frame and scrolls inside it at every width", () => {
    const scrim = baseRule(".scrim");
    expect(declaration(scrim.body, "padding-top")).toBe("9vh");
    expect(declaration(scrim.body, "padding-bottom")).toBe("9vh");

    const sheet = baseRule(".sheet");
    expect(declaration(sheet.body, "max-height")).toBe("100%");
    expect(declaration(sheet.body, "overflow-y")).toBe("auto");

    // And the phone block does not bound it a second time, to a number that
    // would once more be taller than the room the scrim leaves.
    for (const narrowed of rulesFor(".sheet").filter((rule) => rule.at !== sheet.at)) {
      expect(declaration(narrowed.body, "max-height")).toBeNull();
    }
  });
});

describe("the console toggle on a phone", () => {
  const phoneRule = (selector) => rulesFor(selector).find((rule) => enclosingAtRule(rule.at) === PHONE_QUERY);

  // At 390x844 with a workspace open the conversation panel was laid over the
  // view column to the bottom of #view at z-index 30 — and the console is a row
  // of #view, not of the body the panel covers. The composer bar landed exactly
  // on #console-toggle, so the console could not be opened at all; with it open
  // the New agent picker, which sits at the foot of the panel, sat on it
  // instead. The panel stops above whatever the console is showing.
  it("leaves the console's row uncovered by the conversation panel", () => {
    const collapsed = phoneRule("#view:has(#console-region[data-size]) .rail-panel");
    const half = phoneRule('#view:has(#console-region[data-size="half"]) .rail-panel');
    expect(declaration(collapsed.body, "bottom")).toBe("var(--console-bar)");
    expect(declaration(half.body, "bottom")).toBe("var(--console-half)");

    // The room reserved is the room the console takes, stated once each.
    expect(declaration(baseRule("#console-region").body, "height")).toBe("var(--console-bar)");
    expect(declaration(baseRule('#console-region[data-size="half"]').body, "height")).toBe("var(--console-half)");

    // A surface with no console mounted at all keeps the whole column: the
    // reservation hangs off the console region being there.
    expect(declaration(phoneRule(".rail-panel").body, "bottom")).toBe("0");
  });
});

describe("the waiting screen's foot", () => {
  // Nothing styles a bare `.row`, so the line that says what the app is doing
  // sat against the frame with the ⟳ clipped at its left edge and the two
  // buttons crowded onto the end of the sentence. It is laid out as a row: the
  // sentence takes the free width, the buttons keep theirs, and they wrap under
  // it rather than squeezing it where there is no room.
  it("is a row that wraps rather than crowding its sentence", () => {
    const row = baseRule(".wait-row");
    expect(declaration(row.body, "display")).toBe("flex");
    expect(declaration(row.body, "align-items")).toBe("center");
    expect(declaration(row.body, "flex-wrap")).toBe("wrap");
    expect(declaration(row.body, "gap")).toBe("10px 12px");

    expect(declaration(baseRule(".wait-row .dim").body, "flex")).toBe("1 1 auto");
    expect(declaration(baseRule(".wait-row .btn").body, "flex")).toBe("none");
  });
});
