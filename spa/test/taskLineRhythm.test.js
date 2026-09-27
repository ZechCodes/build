// The timeline's gap and the variable the task lines pull back against are
// one number, declared together — everywhere.
//
// #40 asked for consecutive notice and action lines to sit about 4px apart.
// They sat 8px apart on the real build, because the rhythm is written as
// `margin-top: calc(4px - var(--thread-gap))` and three rules set the gap
// while a fourth set only the variable: the rail's gap is 22px and the
// variable said 18, so the pull-back was 4px short.
//
// Nothing could catch that. jsdom has no layout, so the computed gap does not
// exist there at all; the browser pass is what found it. What CAN be checked
// without layout is the invariant underneath it — that no rule sets one half
// of the pair — and that is what this file does, so the next rule to touch the
// timeline's spacing cannot reintroduce it silently.

import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";

/** Every CSS file the SPA ships, read as text. */
function stylesheets() {
  const roots = [resolve(process.cwd(), "src"), resolve(process.cwd(), "src/styles")];
  const found = [];
  for (const root of roots) {
    for (const name of readdirSync(root)) {
      if (name.endsWith(".css")) found.push([join(root, name), readFileSync(join(root, name), "utf8")]);
    }
  }
  return found;
}

/** Every rule whose selector names the timeline, as `{ where, selector, body }`. */
function timelineRules() {
  const rules = [];
  for (const [where, css] of stylesheets()) {
    const pattern = /([^{}]*\.thread-timeline[^{}]*)\{([^}]*)\}/g;
    for (let match = pattern.exec(css); match; match = pattern.exec(css)) {
      rules.push({ where, selector: match[1].trim(), body: match[2] });
    }
  }
  return rules;
}

describe("the timeline's gap", () => {
  it("is declared somewhere at all", () => {
    const setting = timelineRules().filter((rule) => /(^|[;\s])gap\s*:/.test(rule.body));
    expect(setting.length).toBeGreaterThan(0);
  });

  // The pair, both ways: a rule that sets the gap must say what the number is
  // for the lines, and a rule that sets the number must be the one setting the
  // gap — a variable declared on its own is a copy that goes stale.
  it("is never set without the variable the task lines read", () => {
    for (const rule of timelineRules()) {
      const setsGap = /(^|[;\s])gap\s*:/.test(rule.body);
      const setsVar = /--thread-gap\s*:/.test(rule.body);
      if (!setsGap && !setsVar) continue;
      expect([rule.where, rule.selector, setsGap, setsVar]).toEqual([rule.where, rule.selector, true, true]);
    }
  });

  it("is always the variable's own value, so the two cannot disagree", () => {
    for (const rule of timelineRules()) {
      const gap = rule.body.match(/(?:^|[;\s])gap\s*:\s*([^;]+)/);
      if (!gap) continue;
      expect([rule.where, rule.selector, gap[1].trim()]).toEqual([rule.where, rule.selector, "var(--thread-gap)"]);
    }
  });
});

describe("the rhythm the rows are drawn with", () => {
  const read = (file) => readFileSync(resolve(process.cwd(), file), "utf8");

  // One rule, against the class every quiet row carries — the lines AND the
  // tool-call rows between them (#52). Named by kind, it pulled two of the
  // three kinds together and left the third a full message gap away.
  it("pulls consecutive quiet rows back against that one number", () => {
    expect(read("src/styles.css")).toContain(
      ".thread-timeline > .thread-quiet-row + .thread-quiet-row { margin-top:calc(4px - var(--thread-gap)); }",
    );
  });

  it("is not also written by kind somewhere else", () => {
    for (const [where, css] of stylesheets()) {
      const byKind = [...css.matchAll(/([^{}]*\+[^{}]*)\{([^}]*margin-top[^}]*)\}/g)]
        .filter((rule) => !rule[1].includes("thread-quiet-row") && /thread-(task-line|activity)/.test(rule[1]));
      expect([where, byKind.map((rule) => rule[1].trim())]).toEqual([where, []]);
    }
  });

  // The rule that started this: tasks.css declared the variable without the
  // gap, so it quietly won the cascade and set the wrong number.
  it("does not declare the number itself", () => {
    expect(read("src/styles/tasks.css")).not.toMatch(/--thread-gap\s*:/);
  });
});
