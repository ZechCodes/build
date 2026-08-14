// @vitest-environment jsdom
// A pre-redesign `term-<n>` URL: the surface it opens is the branch, and the
// terminal it named is handed to that branch's console. The canonical URL has
// nowhere to keep it — the console is not a tab — so the hand-off happens where
// the URL is read, and survives the rewrite the resolve hop performs.

import { describe, it, expect, beforeEach } from "vitest";

const { App, initRouter } = await import("../src/app.js");
const { takeConsoleTerminal } = await import("../src/core/consoleModel.js");

const hashChanged = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  App.gated = true; // the gate owns #root: a hash change parses, and renders nothing
  takeConsoleTerminal();
});

describe("reading a URL that names a terminal", () => {
  it("parks the terminal for the console that is about to mount", () => {
    location.hash = "#/task/run-1/term-2";
    initRouter();
    expect(App.route).toEqual({ name: "resolve", kind: "run", id: "run-1", tab: "changes", term: "term-2" });
    expect(takeConsoleTerminal()).toBe("term-2");
  });

  it("parks nothing for a URL that names no terminal", async () => {
    location.hash = "#/project/p/branch/main/changes";
    await hashChanged();
    expect(takeConsoleTerminal()).toBeNull();
  });
});
