// @vitest-environment jsdom
// The app has ONE button vocabulary, and two places broke out of it.
//
// The branch surface's Done is a split button whose primary carried a
// `danger` class: the accent background of .btn.primary under the red text of
// .btn.danger, which rendered a green button with red words. What a
// destructive verb costs belongs in the confirmation it opens, not in the
// button's colors — so no rule may paint a split button's primary text
// anything but --accent-ink.
//
// The git bar (Fetch / Pull / Push / Stash) had its own bespoke metrics and
// palette layered over .gittoolbar .btn. It uses the shared .btn.mini
// vocabulary now; only the ahead/behind chips keep colors of their own.
//
// The CSS side is read as text — the rules are the contract, and jsdom's
// matches() answers which of them would land on the markup the builders emit.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { mountSplitButton, splitButtonMarkup } from "../src/core/splitButton.js";
import { gitToolbarHtml } from "../src/core/gitRender.js";
import { pullSplitOptions } from "../src/core/gitPane.js";

const sheet = (name) => readFileSync(resolve("src", name), "utf8");

// All three sheets in load order — a rule in any of them can land on a button.
const stylesSource = [sheet("styles/shell.css"), sheet("styles.css"), sheet("styles/surfaces.css")]
  .join("\n")
  .replace(/\/\*[\s\S]*?\*\//g, "");

/** Every `selector { declarations }` pair in a sheet, at-rule preludes skipped
 *  (their inner rules match on their own) — the paneLayout.test.js idiom. */
function rulesIn(source) {
  return [...source.matchAll(/([^{}@;]+)\{([^{}]*)\}/g)].map((match) => ({
    selector: match[1].trim().replace(/\s+/g, " "),
    body: match[2].trim(),
  }));
}

/** The value a rule body settles on for one property, or null. Names are
 *  compared whole, so `color` never reads out of `border-color`. */
function declaration(body, property) {
  let found = null;
  for (const piece of body.split(";")) {
    const colon = piece.indexOf(":");
    if (colon < 0) continue;
    if (piece.slice(0, colon).trim() === property) found = piece.slice(colon + 1).trim();
  }
  return found;
}

// A state is a state of the same element, so :hover/:focus/:active rules are
// asked about the element too — a red-on-hover primary is the same collision.
const STATE_PSEUDO = /:(hover|focus-visible|focus|active|disabled)\b/g;

/** A selector's specificity as one comparable number. `:not()` contributes its
 *  argument, so the parentheses are unwrapped before counting. */
function specificityOf(selector) {
  const flat = selector.replace(/:not\(|\)/g, " ");
  const ids = (flat.match(/#[\w-]+/g) || []).length;
  const classes = (flat.match(/[.:[]/g) || []).length;
  const elements = (flat.match(/(^|[\s>+~])[a-z][\w-]*/gi) || []).length;
  return ids * 10000 + classes * 100 + elements;
}

/** Every declaration of `property` in the sheets whose selector would land on
 *  `element`, in source order. State pseudo-classes are stripped before asking
 *  jsdom, so a rule that only bites on :hover is read as landing too — a
 *  primary that turns red under the pointer is the same collision. Pseudo-
 *  element rules are skipped (they paint generated content, not the element's
 *  own text), as are selector parts jsdom cannot parse. */
function matchedDeclarations(element, property) {
  const matched = [];
  for (const rule of rulesIn(stylesSource)) {
    const value = declaration(rule.body, property);
    if (value === null) continue;
    for (const part of rule.selector.split(",")) {
      const selector = part.trim().replace(STATE_PSEUDO, "");
      if (!selector || selector.includes("::")) continue;
      let lands = false;
      try {
        lands = element.matches(selector);
      } catch {
        continue;
      }
      if (lands) matched.push({ selector: part.trim(), value, specificity: specificityOf(selector) });
    }
  }
  return matched;
}

/** What the cascade settles `property` on for `element`: the most specific
 *  landing rule, last one winning a tie. */
function settledValue(element, property) {
  const matched = matchedDeclarations(element, property);
  expect(matched.length, `no rule sets ${property} on ${element.className}`).toBeGreaterThan(0);
  return matched.reduce((winner, rule) => (rule.specificity >= winner.specificity ? rule : winner)).value;
}

const render = (markup) => {
  document.body.innerHTML = markup;
  return document.body;
};

const DONE_OPTION = {
  id: "finish_delete",
  label: "Done",
  menuLabel: "Done — delete the branch",
  description: "delete branch build/login and its checkout",
  busyLabel: "deleting…",
  danger: true,
};

describe("a split button's primary is the app's primary button", () => {
  it("settles the destructive primary's text on --accent-ink, and no rule proposes another", () => {
    render(splitButtonMarkup([DONE_OPTION, { id: "other", menuLabel: "Other", description: "d", busyLabel: "…" }]));
    for (const button of document.querySelectorAll(".splitbtn .btn")) {
      expect(settledValue(button, "color")).toBe("var(--accent-ink)");
      // The base .btn's var(--ink) is the vocabulary; anything red is the
      // destructive palette leaking into a primary button's chrome.
      for (const { selector, value } of matchedDeclarations(button, "color"))
        expect(`${selector} → ${value}`).not.toContain("--red");
    }
  });

  it("settles the destructive primary on the accent background", () => {
    render(splitButtonMarkup([DONE_OPTION]));
    const primary = document.querySelector(".splitbtn .btn:not(.caret)");
    expect(settledValue(primary, "background")).toBe("var(--accent-fill)");
    expect(settledValue(primary, "border-color")).toBe("var(--accent-fill)");
  });

  it("keeps the standard border and radius, so it reads like every other button", () => {
    render(splitButtonMarkup([DONE_OPTION]));
    const primary = document.querySelector(".splitbtn .btn:not(.caret)");
    expect(settledValue(primary, "border")).toBe("1px solid var(--line)");
    expect(settledValue(primary, "border-radius")).toBe("9px");
  });
});

describe("the git bar speaks the shared button vocabulary", () => {
  it("gives Fetch the shared mini classes", () => {
    render(gitToolbarHtml({ chips: { ahead: 0, behind: 0 } }));
    const fetch = document.querySelector(".gtfetch");
    expect(fetch.classList.contains("btn")).toBe(true);
    expect(fetch.classList.contains("mini")).toBe(true);
  });

  it("mounts Pull as a mini split button — shared classes, dropdown intact", () => {
    render(gitToolbarHtml({ chips: null }));
    const host = document.querySelector(".gtpull");
    mountSplitButton(host, { options: pullSplitOptions(), run: async () => {}, variant: "mini" });
    const buttons = [...host.querySelectorAll(".btn")];
    expect(buttons.length).toBe(2);
    for (const button of buttons) {
      expect(button.classList.contains("btn")).toBe(true);
      expect(button.classList.contains("mini")).toBe(true);
      expect(button.classList.contains("primary")).toBe(false);
    }
    expect(host.querySelector(".splitmenu")).toBeTruthy();
    expect(host.querySelectorAll(".splitmenu .mi").length).toBe(pullSplitOptions().length);
  });

  it("leaves the git bar's buttons to the shared rules — no look of its own", () => {
    const ownLook = ["background", "color", "padding", "border", "border-color", "border-radius", "min-height", "box-shadow"];
    const bespoke = rulesIn(stylesSource)
      .filter((rule) => /\.gittoolbar\b[^,{]*\.btn\b/.test(rule.selector))
      .flatMap((rule) => ownLook.filter((property) => declaration(rule.body, property) !== null).map((property) => `${rule.selector} { ${property} }`));
    expect(bespoke).toEqual([]);
  });

  it("keeps the ahead/behind chips' own colors", () => {
    render(gitToolbarHtml({ chips: { ahead: 2, behind: 1 } }));
    expect(settledValue(document.querySelector(".gtahead"), "color")).toBe("var(--accent)");
    expect(settledValue(document.querySelector(".gtbehind"), "color")).toBe("var(--amber)");
  });
});
