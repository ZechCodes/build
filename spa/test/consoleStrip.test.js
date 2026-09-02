// @vitest-environment jsdom
// The console head as a strip of open terminals.
//
// The head is a label that opens and shuts the panel, a horizontally scrolling
// strip of one tab per terminal with the + riding sticky at its right edge, and
// the grow control on the far right. The strip is a keyed list: a tab arrives
// by growing into the row and leaves by shrinking out of it, and everything
// else — a selection, a size — patches the tabs already standing.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { MOTION_DURATION_MS } from "../src/core/motion.js";
import { recordAnimations, settleMotion, stopRecordingAnimations } from "./motionRecorder.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const shellCss = readFileSync(resolve("src/styles/shell.css"), "utf8");

const manager = {
  listTerminals: vi.fn(async () => []),
  createTerminal: vi.fn(async () => ({ term_id: "term-9" })),
  closeTerminal: vi.fn(async () => {}),
  attachTerminal: vi.fn(async () => ({ snapshot: "", cursor: 0 })),
  input: vi.fn(async () => {}),
  resize: vi.fn(async () => {}),
  detach: vi.fn(),
};
vi.mock("../src/terminal/manager.js", () => ({
  terminalManager: () => manager,
  subscribeTerminalStatus: () => () => {},
}));
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { host, opts, dispose() {} };
  },
}));

const { App } = await import("../src/app.js");
const { consoleHeadHtml, mountConsole, resetConsoleMemory } = await import("../src/core/console.js");

const flush = async () => {
  for (let i = 0; i < 6; i++) await new Promise((done) => setTimeout(done, 0));
};

const region = () => document.getElementById("console-region");
const strip = () => region().querySelector(".console-tabs");
const tabElements = () => [...region().querySelectorAll(".console-tab")];
const tabNames = () => [...region().querySelectorAll(".console-tab-name")].map((cell) => cell.textContent);

/** The one element a piece of head markup describes, for reading the pure
 *  renderer's answer as a tree rather than as a string. */
const parse = (html) => {
  const holder = document.createElement("div");
  holder.innerHTML = html;
  return holder;
};

let started = [];
let panel = null;

const openConsole = async (termIds) => {
  manager.listTerminals.mockResolvedValue(termIds.map((term_id) => ({ term_id })));
  panel = mountConsole(region(), { kind: "branch", projectId: "p1", branch: "build/login" });
  await flush();
  region().querySelector(".console-bar").click();
  await flush();
  await settleMotion(started);
};

beforeEach(() => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  resetConsoleMemory();
  started = recordAnimations();
  manager.listTerminals.mockReset().mockResolvedValue([]);
  manager.createTerminal.mockReset().mockResolvedValue({ term_id: "term-9" });
  manager.closeTerminal.mockReset().mockResolvedValue(undefined);
  manager.attachTerminal.mockReset().mockResolvedValue({ snapshot: "", cursor: 0 });
  manager.detach.mockReset();
  App.call = vi.fn(async () => ({ project_id: "p1", branch: "build/login", run_id: "run-3" }));
});

afterEach(async () => {
  if (panel) panel.dispose();
  panel = null;
  await settleMotion(started);
  stopRecordingAnimations();
  started = [];
});

describe("the head's markup", () => {
  it("makes the label itself the way in, with no caret beside it", () => {
    const shut = consoleHeadHtml({ size: "collapsed" });
    expect(shut).toContain('id="console-toggle"');
    expect(shut).toContain('aria-expanded="false"');
    expect(shut).toContain("Console");
    expect(shut).not.toContain("console-caret");
    expect(shut).not.toContain("▼");
    expect(shut).not.toContain("▲");
    expect(consoleHeadHtml({ size: "half" })).toContain('aria-expanded="true"');
  });

  it("hangs the + at the end of the tab strip", () => {
    const head = parse(
      consoleHeadHtml({
        size: "half",
        tabs: [
          { id: "term-1", label: "Terminal 1" },
          { id: "term-2", label: "Terminal 2" },
        ],
        selected: "term-2",
      }),
    );
    const cells = [...head.querySelectorAll(".console-tabs > *")];
    expect(cells.map((cell) => cell.className)).toEqual([
      "console-tab",
      "console-tab active",
      "iconbtn console-new",
    ]);
    expect(head.querySelector(".console-controls .console-grow")).toBeTruthy();
    expect(head.querySelector(".console-controls .console-new")).toBeNull();
  });

  it("offers no + where there is no checkout to open a shell in", () => {
    const head = parse(consoleHeadHtml({ size: "half", tabs: [], selected: null, scoped: false }));
    expect(head.querySelector(".console-tabs")).toBeTruthy();
    expect(head.querySelector(".console-new")).toBeNull();
  });
});

describe("the head's rules", () => {
  it("scrolls the strip sideways with no scrollbar to show for it", () => {
    const rule = shellCss.match(/\.console-tabs \{[^}]*\}/)[0];
    expect(rule).toMatch(/overflow-x:auto/);
    expect(rule).toMatch(/scrollbar-width:none/);
    expect(rule).toMatch(/flex:1/);
    expect(shellCss).toMatch(/\.console-tabs::-webkit-scrollbar \{[^}]*display:none/);
  });

  it("keeps the + at the strip's right edge, over the tabs that scroll under it", () => {
    const rule = shellCss.match(/\.console-tabs \.console-new \{[^}]*\}/)[0];
    expect(rule).toMatch(/position:sticky/);
    expect(rule).toMatch(/right:0/);
    expect(rule).toMatch(/background:var\(--bg\)/);
  });

  it("has no caret left to style", () => {
    expect(shellCss).not.toContain("console-caret");
  });
});

describe("the strip as a keyed list", () => {
  it("grows a new terminal's tab into the row", async () => {
    await openConsole(["term-1"]);
    region().querySelector(".console-new").click();
    await flush();
    expect(tabNames()).toEqual(["Terminal 1", "Terminal 2"]);
    const arriving = region().querySelector('.console-tab[data-key="term-9"]');
    const move = started.find((run) => run.element === arriving);
    expect(move.keyframes[0].width).toBe("0px");
    expect(move.options.duration).toBe(MOTION_DURATION_MS);
  });

  it("keeps the + last, after the tab that just arrived", async () => {
    await openConsole([]);
    region().querySelector(".console-new").click();
    await flush();
    const cells = [...strip().children];
    expect(cells[cells.length - 1].className).toContain("console-new");
    expect(cells[cells.length - 2].dataset.key).toBe("term-9");
  });

  it("shrinks a closed terminal's tab out of the row before it leaves", async () => {
    await openConsole(["term-1", "term-2"]);
    const leaving = region().querySelector('.console-tab[data-key="term-1"]');
    region().querySelector('[data-close="term-1"]').click();
    await flush();
    expect(leaving.parentNode).toBe(strip());
    expect(leaving.hasAttribute("data-exiting")).toBe(true);
    const move = started.filter((run) => run.element === leaving).pop();
    expect(move.keyframes[1].width).toBe("0px");
    await settleMotion(started);
    expect(region().querySelector('.console-tab[data-key="term-1"]')).toBeNull();
  });

  it("selects a tab without rebuilding the strip", async () => {
    await openConsole(["term-1", "term-2"]);
    const before = tabElements();
    before[1].querySelector(".console-tab-name").click();
    await flush();
    const after = tabElements();
    expect(after[0]).toBe(before[0]);
    expect(after[1]).toBe(before[1]);
    expect(after[1].className).toContain("active");
    expect(after[0].className).not.toContain("active");
  });

  it("scrolls the strip to the newest tab when a terminal is opened", async () => {
    await openConsole(["term-1"]);
    let scrolled = 0;
    Object.defineProperty(strip(), "scrollWidth", { configurable: true, get: () => 640 });
    Object.defineProperty(strip(), "scrollLeft", {
      configurable: true,
      get: () => scrolled,
      set: (value) => {
        scrolled = value;
      },
    });
    region().querySelector(".console-new").click();
    await flush();
    expect(scrolled).toBe(640);
  });
});
