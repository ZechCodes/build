/** @vitest-environment jsdom */
// #52: the quiet rows are ONE list — a notice, a task action, a tool call,
// and a folded run of tool calls.
//
// The maintainer: "Activity entries should be styled like the notification
// entries … just same padding so they all group nicely together."
//
// They were not. Measured in a browser at 390px, the notice and action lines
// were 19px tall and sat 4px apart; the tool-call row was 28px tall — two
// lines — and sat 22px from the line above and below it, because the rhythm
// rule named one kind (`.thread-task-line + .thread-task-line`) and the box
// was written twice, once per kind, with different numbers.
//
// So the two things a row of either kind needs — its box and its pull-back —
// are stated ONCE apiece, against a class every quiet row carries. These tests
// hold that shape: the class is on every kind (and on nothing else), and no
// rule sets a quiet row's vertical box by kind behind the shared one's back.
// None of this can measure the gap — jsdom has no layout, and that is what
// web/task-line-measure.mjs is for — but it can keep the two kinds from
// drifting apart again the way they did.

import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { threadHtml } from "../src/core/thread.js";

/** The class every quiet row carries, and the only thing the box and the
 *  rhythm are written against. */
const QUIET_ROW = "thread-quiet-row";

const place = { deviceId: "dev-1", projectId: "proj-1", projectName: "Build" };

const notice = (sequence) => ({
  type: "message",
  data: {
    id: `m-${sequence}`, sequence, role: "user", from_build: true,
    from_task: { task_id: `task-${sequence}`, number: sequence, title: "A notice" },
    task_notice: { actor: "user", action: "commented" }, body: "",
  },
});
const action = (sequence) => ({
  type: "message",
  data: {
    id: `m-${sequence}`, sequence, role: "agent", body: "",
    task_action: { task_id: `task-${sequence}`, number: sequence, title: "An action", action: "closed" },
  },
});
const toolCall = (sequence) => ({
  type: "event",
  data: { event: "tool_use", sequence, summary: "Bash npm run lint", created_at: "2026-09-20T21:10:00Z" },
});
const said = (sequence) => ({ type: "message", data: { id: `m-${sequence}`, sequence, role: "user", body: "Words." } });

/** One timeline, painted. `openRuns` opens every folded run, which is what
 *  puts a bare `.thread-activity` row on the page beside the group head — both
 *  are quiet rows and both are asked about below. */
function painted(items) {
  const host = document.createElement("div");
  host.innerHTML = threadHtml(
    { id: "c-1", items },
    { place, openRuns: { has: () => true }, runItemsOf: () => undefined },
  );
  return host;
}

describe("the class every quiet row carries", () => {
  // A table, because the point is that no kind is missing from it: the row
  // that gets forgotten is the one that drifts.
  const KINDS = [
    { name: "a notice about a task", items: [notice(41)], selector: ".thread-task-line.thread-notice" },
    { name: "an action line", items: [action(42)], selector: ".thread-task-line.thread-action" },
    { name: "a folded run of tool calls", items: [toolCall(43)], selector: ".thread-activity-group" },
    { name: "a tool call", items: [toolCall(44), toolCall(45)], selector: ".thread-activity" },
  ];

  for (const { name, items, selector } of KINDS) {
    it(`is on ${name}`, () => {
      const rows = [...painted(items).querySelectorAll(selector)];
      expect(rows.length).toBeGreaterThan(0);
      for (const row of rows) expect([selector, row.className]).toEqual([selector, expect.stringContaining(QUIET_ROW)]);
    });
  }

  // The rhythm pulls consecutive quiet rows together, so anything wearing the
  // class is something the reader scans past. A message is not: it keeps the
  // full gap, because it IS a separate thing to say.
  it("is on nothing a person said", () => {
    const host = painted([said(1), notice(2), said(3)]);
    for (const message of host.querySelectorAll(".thread-message.user, .thread-message.from-agent")) {
      expect(message.className).not.toContain(QUIET_ROW);
    }
  });
});

/** Every CSS file the SPA ships, as `{ where, selector, body }` rules. */
function rules() {
  const found = [];
  for (const root of [resolve(process.cwd(), "src"), resolve(process.cwd(), "src/styles")]) {
    for (const name of readdirSync(root)) {
      if (!name.endsWith(".css")) continue;
      // Comments go first: this file's own prose is full of braces and class
      // names, and a rule reported with a paragraph in front of it is a
      // failure nobody can read.
      const css = readFileSync(join(root, name), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
      const pattern = /([^{}]*)\{([^}]*)\}/g;
      for (let match = pattern.exec(css); match; match = pattern.exec(css)) {
        found.push({ where: name, selector: match[1].trim(), body: match[2] });
      }
    }
  }
  return found;
}

/** The classes that name a quiet row or the line inside one. A rule whose
 *  SUBJECT is one of these is a rule about a quiet row's box. */
const ROW_CLASSES = [
  "thread-task-line", "thread-task-notice", "thread-task-action",
  "thread-activity", "thread-activity-head", "thread-activity-group", "thread-activity-group-head",
];

/** The classes the last compound of `selector` names — what the rule is about,
 *  as opposed to what it is nested under. */
const subjectClasses = (selector) => {
  const last = selector.trim().split(/[\s>+~]+/).filter(Boolean).pop() || "";
  return [...last.matchAll(/\.([A-Za-z0-9_-]+)/g)].map((one) => one[1]);
};

const SETS_VERTICAL_BOX = /(^|[;\s])(min-height|padding|padding-top|padding-bottom|padding-block)\s*:/;

describe("the quiet row's box", () => {
  it("is set by the shared class and by no kind behind its back", () => {
    const byKind = rules().filter((rule) =>
      SETS_VERTICAL_BOX.test(rule.body)
      && rule.selector.split(",").some((one) =>
        !one.includes(QUIET_ROW) && subjectClasses(one).some((name) => ROW_CLASSES.includes(name))));
    expect(byKind.map((rule) => `${rule.where}  ${rule.selector}`)).toEqual([]);
  });

  it("is set by the shared class somewhere", () => {
    const shared = rules().filter((rule) =>
      SETS_VERTICAL_BOX.test(rule.body)
      && rule.selector.split(",").some((one) => subjectClasses(one).includes(QUIET_ROW)));
    expect(shared.length).toBeGreaterThan(0);
  });
});
