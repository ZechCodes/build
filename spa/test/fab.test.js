// @vitest-environment jsdom
// The create FAB: an issue verb with a worktree verb ranked under it.

import { describe, it, expect } from "vitest";
import { fabHtml, mountFab } from "../src/core/fab.js";

const mount = (handlers = {}) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  mountFab(host, handlers);
  return {
    host,
    issue: host.querySelector('[data-fab="issue"]'),
    worktree: host.querySelector('[data-fab="worktree"]'),
  };
};

describe("fabHtml", () => {
  it("renders both verbs, the issue one primary", () => {
    const html = fabHtml();
    expect(html).toContain('data-fab="issue"');
    expect(html).toContain("New issue");
    expect(html).toContain('data-fab="worktree"');
    expect(html).toContain('aria-label="New worktree"');
  });

  // DOM order IS tab order, and the stack is column-reverse, so the primary
  // coming first gives both: the main verb is focused first and sits at the
  // bottom of the stack, with the mini rising above it.
  it("puts the primary FAB first in DOM order", () => {
    const html = fabHtml();
    expect(html.indexOf('data-fab="issue"')).toBeLessThan(html.indexOf('data-fab="worktree"'));
  });

  it("escapes caller-supplied labels", () => {
    const html = fabHtml({ issueLabel: '<img src=x onerror="alert(1)">' });
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
  });
});

describe("mountFab", () => {
  it("reports the issue verb", () => {
    const calls = [];
    const fab = mount({ onNewIssue: () => calls.push("issue") });
    fab.issue.click();
    expect(calls).toEqual(["issue"]);
  });

  it("single-flights the worktree verb — a second click cannot cut a second branch", async () => {
    let settle;
    const gate = new Promise((resolve) => (settle = resolve));
    let calls = 0;
    const fab = mount({
      onNewWorktree: () => {
        calls += 1;
        return gate;
      },
    });
    fab.worktree.click();
    fab.worktree.click();
    expect(calls).toBe(1);
    expect(fab.worktree.disabled).toBe(true);
    settle();
    await gate;
    await Promise.resolve();
    expect(fab.worktree.disabled).toBe(false);
  });

  it("re-enables the worktree verb when the call fails", async () => {
    const fab = mount({ onNewWorktree: () => Promise.reject(new Error("no space left")) });
    fab.worktree.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(fab.worktree.disabled).toBe(false);
  });
});
