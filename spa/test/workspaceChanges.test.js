// @vitest-environment jsdom
// The Changes tab row's markup (#174): one tab per workspace directory, git or
// not, the current one selected and the row's one tab stop.

import { describe, expect, it } from "vitest";
import { directoryHasGit, directoryTabsHtml } from "../src/views/workspaceChanges.js";

const paint = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
};

describe("the Changes tab row", () => {
  it("is a tablist of every directory, the current one selected and reachable by Tab", () => {
    const host = paint(directoryTabsHtml([
      { sourceId: "repo", label: "Repository", current: false },
      { sourceId: "assets", label: "Assets", current: true },
    ]));
    const row = host.querySelector("[role=tablist]");
    expect(row.getAttribute("aria-label")).toBe("Workspace directories");
    expect([...row.querySelectorAll("[role=tab]")].map((tab) => [tab.dataset.directory, tab.textContent, tab.getAttribute("aria-selected"), tab.tabIndex]))
      .toEqual([["repo", "Repository", "false", -1], ["assets", "Assets", "true", 0]]);
  });

  it("escapes names and ids, which come from the user and the repos", () => {
    const hostile = '<img src=x onerror=alert(1)>" onfocus="x';
    const html = directoryTabsHtml([{ sourceId: hostile, label: hostile, current: true }]);
    expect(html).not.toContain("<img");
    const tab = paint(html).querySelector("[role=tab]");
    expect(tab.dataset.directory).toBe(hostile);
    expect(tab.textContent).toBe(hostile);
  });

  it("treats only a record saying so as a directory without git", () => {
    expect([directoryHasGit({ is_git: false }), directoryHasGit({ is_git: true }), directoryHasGit({})]).toEqual([false, true, true]);
  });
});
