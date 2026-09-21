/** @vitest-environment jsdom */
// The markup web/issue-line-measure.mjs measures, kept honest.
//
// That script runs in a browser against the real stylesheet, which is the only
// place a gutter, a wrap or a 4px gap exists at all — jsdom has no layout, so
// every one of #40's defects passed the suite. What it measures has to be the
// renderer's own output, not a hand-copied imitation that quietly stops
// matching it.
//
// So the output is committed (web/fixtures/issue-lines.html) and this asserts
// it is still what the renderer produces. The browser script then needs no
// build tool and no live bridge to produce its rows; if the renderer changes,
// this fails and says to regenerate, rather than the measurement silently
// checking markup the product no longer emits.
//
// Regenerate with: ISSUE_LINE_FIXTURE=write npx vitest run test/issueLineFixture.test.js

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { threadHtml } from "../src/core/thread.js";

const FIXTURE = resolve(process.cwd(), "../web/fixtures/issue-lines.html");

const place = { deviceId: "dev-1", projectId: "proj-1", projectName: "Build" };
const agentLabels = { "agent-01M2A": "issues-spa · Agent 1" };

const message = (sequence, body) => ({ type: "message", data: { id: `m-${sequence}`, sequence, role: "agent", body } });
const notice = (sequence, title, issue_notice) => ({
  type: "message",
  data: {
    id: `m-${sequence}`, sequence, role: "user", from_build: true,
    from_issue: { issue_id: `issue-${sequence}`, number: sequence, title },
    issue_notice, body: "",
  },
});
const action = (sequence, title, act) => ({
  type: "message",
  data: {
    id: `m-${sequence}`, sequence, role: "agent", body: "",
    issue_action: { issue_id: `issue-${sequence}`, number: sequence, title, action: act },
  },
});

/**
 * A run of lines between two ordinary messages.
 *
 * Deliberately mixed: both row kinds, all three actor kinds, a title far too
 * long for a phone and one short enough to need no ellipsis, and the raw
 * `commented_on` token the bridge actually sends. The measurement is only
 * worth taking over rows that cover what goes wrong.
 */
export const ISSUE_LINE_ITEMS = [
  message(0, "A normal message before the run, so the gap either side can be measured."),
  // #42's four kinds, so one screenshot shows all of them side by side: the
  // reader's own message, another agent's, Build's own notice, and a notice
  // about an issue.
  { type: "message", data: { id: "m-u", sequence: 2, role: "user", body: "Take the retry path next, and tell me what you find." } },
  {
    type: "message",
    data: {
      id: "m-a", sequence: 3, role: "user",
      from_agent: { id: "project-01M2SCB", owner: { kind: "project", id: "proj-1", name: "Build" }, topic: "Verify per-file roll" },
      body: Array.from({ length: 12 }, (_, line) => `line ${line + 1} of the project agent's brief`).join("\n"),
    },
  },
  {
    type: "message",
    data: {
      id: "m-b", sequence: 4, role: "user", from_build: true,
      body: "Build restarted at 2026-09-20T20:09:27.317756819Z (bridge 0.2.0) and brought your session back. This message is from Build, not from the user — nobody is waiting on an answer to it. Assume nothing you were doing finished.",
    },
  },
  notice(41, "Tracking notices render as one deep-linked line, X did Y on N Title, not as a user message", { actor: "project-01M2SCB", action: "commented" }),
  notice(42, "Issues list shows open issues by default", { actor: "agent-01M2A", action: "moved to In review" }),
  action(43, "Issue notice and action lines: one line, full width, no gutter, tight spacing", "commented_on"),
  action(44, "A short one", "created"),
  // #52: an activity row in the middle of the run. Zech: "Activity entries
  // should be styled like the notification entries … just same padding so
  // they all group nicely together." Between two lines, so the measurement
  // can ask whether the rhythm survives it.
  { type: "event", data: { event: "tool_use", sequence: 441, summary: "Bash cd spa && npm run lint", created_at: "2026-09-20T21:10:00Z" } },
  notice(45, "Workspace issues as a tab of the workspace page, with the workspace rail beside it", { actor: "user", action: "closed" }),
  message(9, "A normal message after the run."),
  // #50: a fenced block far wider than a phone. The block must scroll; the
  // bubble and the column it is in must not grow by a pixel.
  message(
    10,
    [
      "A message carrying code:",
      "",
      "```rust",
      "let manifest = Manifest::from_parts(&workspace_id, &source_id, &revision).expect(\"the manifest was just built\");",
      "```",
    ].join("\n"),
  ),
];

const rendered = () => threadHtml({ id: "c-1", items: ISSUE_LINE_ITEMS }, { place, agentLabels });

// The fixture carries relative times ("1 hour ago") rendered against the
// clock, so the clock is frozen here or the committed markup rots by the hour.
const FROZEN_NOW = new Date("2026-09-20T22:30:00Z");
beforeAll(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(FROZEN_NOW);
});
afterAll(() => vi.useRealTimers());

describe("the markup the browser measurement is taken over", () => {
  it("is what the renderer produces", () => {
    const html = rendered();
    if (process.env.ISSUE_LINE_FIXTURE === "write") {
      mkdirSync(dirname(FIXTURE), { recursive: true });
      writeFileSync(FIXTURE, html);
    }
    expect(readFileSync(FIXTURE, "utf8")).toBe(html);
  });

  // The measurement asks about these rows specifically; a fixture that stopped
  // containing them would measure nothing and still pass.
  it("holds a run of issue lines between ordinary messages", () => {
    const host = document.createElement("div");
    host.innerHTML = rendered();
    expect(host.querySelectorAll(".thread-issue-line")).toHaveLength(6);
    expect(host.querySelectorAll(".thread-action")).toHaveLength(2);
  });

  // #42: one screenshot has to show all four, or it cannot show that no two
  // of them look alike.
  // #50's subject: the measurement needs a line wider than any phone.
  it("holds a fenced block too wide for a phone", () => {
    const host = document.createElement("div");
    host.innerHTML = rendered();
    const block = host.querySelector("pre.md-code");
    expect(block).not.toBeNull();
    expect(block.textContent.split("\n")[0].length).toBeGreaterThan(100);
  });

  // #52's subject: the row has to sit in the run, not beside it.
  it("holds an activity row between two issue lines", () => {
    const host = document.createElement("div");
    host.innerHTML = rendered();
    const kinds = [...host.querySelectorAll(".thread-issue-line, .thread-activity, .thread-activity-group")]
      .map((one) => (one.classList.contains("thread-issue-line") ? "line" : "activity"));
    expect(kinds).toContain("activity");
    // Surrounded, so a gap either side of it is measurable.
    const at = kinds.indexOf("activity");
    expect([kinds[at - 1], kinds[at + 1]]).toEqual(["line", "line"]);
  });

  it("holds all four kinds at once", () => {
    const host = document.createElement("div");
    host.innerHTML = rendered();
    expect(host.querySelectorAll(".thread-message.user")).not.toHaveLength(0);
    expect(host.querySelectorAll(".thread-message.from-agent")).not.toHaveLength(0);
    // Build's own notice, and a notice about an issue.
    expect(host.querySelectorAll(".thread-notice details.thread-notice-more")).not.toHaveLength(0);
    expect(host.querySelectorAll(".thread-notice [data-issue-notice]")).not.toHaveLength(0);
  });
});
