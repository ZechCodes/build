// @vitest-environment jsdom
// Where a conversation opens, and where it stays.
//
// The thread is a timeline: the newest message is the one the human came for,
// and it sits at the BOTTOM. Every surface re-renders the whole thread on its
// poll, and innerHTML resets scrollTop, so "open at the newest" and "leave a
// reader in the history they scrolled to" are the same problem — both are the
// scroll position around a repaint.

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { paintThreadKeepingPlace, threadHtml } from "../src/core/thread.js";
import { mountPrimaryConversation } from "../src/views/mainWorktree.js";
import { createPrimaryAdoptingCall } from "../src/core/adoption.js";

/** jsdom has no layout, so the scroller states its own geometry: a viewport
 *  short enough that the thread overflows it. */
function scroller({ scrollHeight = 1000, clientHeight = 300, scrollTop = 0 } = {}) {
  const element = document.createElement("div");
  Object.defineProperty(element, "scrollHeight", { get: () => scrollHeight, configurable: true });
  Object.defineProperty(element, "clientHeight", { get: () => clientHeight, configurable: true });
  element.scrollTop = scrollTop;
  document.body.appendChild(element);
  return element;
}

const paintThread = (host) => () => {
  host.innerHTML = threadHtml({ items: [{ type: "message", data: { role: "agent", body: "landed" } }] });
};

describe("a conversation opens at its newest message", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("scrolls to the bottom on the paint that opens the tab", () => {
    const body = scroller();
    paintThreadKeepingPlace(body, paintThread(body));
    expect(body.scrollTop).toBe(1000);
  });

  it("keeps the bottom pinned when the reader is already there", () => {
    const body = scroller();
    paintThreadKeepingPlace(body, paintThread(body));
    body.scrollTop = 700; // scrollHeight - clientHeight: at the bottom
    paintThreadKeepingPlace(body, paintThread(body));
    expect(body.scrollTop).toBe(1000);
  });

  it("leaves a reader who scrolled up exactly where they were", () => {
    const body = scroller();
    paintThreadKeepingPlace(body, paintThread(body));
    body.scrollTop = 120;
    paintThreadKeepingPlace(body, paintThread(body));
    expect(body.scrollTop).toBe(120);
  });

  it("paints and does nothing else when there is no scroll container", () => {
    const host = document.createElement("div");
    paintThreadKeepingPlace(null, paintThread(host));
    expect(host.querySelector(".review-thread")).toBeTruthy();
  });

  it("paints the thread whether or not it moves the scroller", () => {
    const body = scroller();
    paintThreadKeepingPlace(body, paintThread(body));
    expect(body.querySelector(".review-thread")).toBeTruthy();
  });

  it("re-pins the bottom on the next frame, so late layout cannot land short", () => {
    const frames = [];
    const original = globalThis.requestAnimationFrame;
    globalThis.requestAnimationFrame = (callback) => frames.push(callback);
    try {
      let height = 1000;
      const body = document.createElement("div");
      Object.defineProperty(body, "scrollHeight", { get: () => height, configurable: true });
      Object.defineProperty(body, "clientHeight", { get: () => 300, configurable: true });
      body.scrollTop = 0;
      document.body.appendChild(body);
      paintThreadKeepingPlace(body, paintThread(body));
      expect(body.scrollTop).toBe(1000);
      height = 1400; // markdown/fonts settled a frame late
      frames.forEach((frame) => frame());
      expect(body.scrollTop).toBe(1400);
    } finally {
      globalThis.requestAnimationFrame = original;
    }
  });

  it("does not chase a reader who scrolls away between the paint and the frame", () => {
    const frames = [];
    const original = globalThis.requestAnimationFrame;
    globalThis.requestAnimationFrame = (callback) => frames.push(callback);
    try {
      const body = scroller();
      paintThreadKeepingPlace(body, paintThread(body)); // opens: bottom
      frames.length = 0;
      body.scrollTop = 700;
      paintThreadKeepingPlace(body, paintThread(body)); // a poll re-render
      expect(frames).toHaveLength(0);
      body.scrollTop = 90; // the reader scrolls up right after the repaint
      frames.forEach((frame) => frame());
      expect(body.scrollTop).toBe(90);
    } finally {
      globalThis.requestAnimationFrame = original;
    }
  });
});

// jsdom rebases import.meta.url onto the fake document location, so the view
// sources are read from the package root the runner starts in.
// One surface driven for real, so the wiring is proved and not just asserted:
// the primary checkout's conversation is the pane that mounts standalone.
describe("a mounted conversation lands on the newest message", () => {
  const runView = (...bodies) => ({
    state: "review",
    harness: "Claude Code",
    thread: {
      items: bodies.map((body, index) => ({
        type: "message",
        data: { role: "agent", body, sequence: index + 1, created_at: "2026-08-06T12:00:00Z" },
      })),
    },
  });

  const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

  it("opens at the bottom and holds a reader who scrolled up through a refresh", async () => {
    const bridge = { view: runView("first") };
    const callRpc = vi.fn(async (method) => (method === "run.get" ? bridge.view : { ok: true }));
    const adopting = createPrimaryAdoptingCall(callRpc, "proj-1");
    adopting.seedAdoptedRun("run-main");
    const body = scroller();

    const pane = mountPrimaryConversation(body, { adopting, callRpc, pollMs: 0 });
    await tick();
    expect(body.textContent).toContain("first");
    expect(body.scrollTop).toBe(1000);

    body.scrollTop = 140;
    bridge.view = runView("first", "second");
    await pane.refresh();
    expect(body.textContent).toContain("second");
    expect(body.scrollTop).toBe(140);
    pane.dispose();
  });
});

const viewSource = (file) => readFileSync(resolve("src/views", file), "utf8");

// The issue view (plan.js) is not here: an issue's conversation belongs to its
// agent, and an agent's conversation lives in the rail beside every surface —
// the issue view paints stage artifacts and no thread of its own.
const SURFACES_WITH_A_CONVERSATION = ["task.js", "worktree.js", "mainWorktree.js"];

describe("every surface's conversation scrolls the same way", () => {
  it.each(SURFACES_WITH_A_CONVERSATION)("%s wraps every thread paint in the shared helper", (file) => {
    const source = viewSource(file);
    const bodyLines = source.split("\n").filter((line) => !/^import\b/.test(line.trim()));
    const threadPaints = bodyLines.join("\n").match(/threadHtml\(/g) || [];
    const wrapped = bodyLines.join("\n").match(/paintThreadKeepingPlace\(/g) || [];
    expect(threadPaints.length).toBeGreaterThan(0);
    expect(wrapped).toHaveLength(threadPaints.length);
  });

  it.each(SURFACES_WITH_A_CONVERSATION)("%s owns no scrolling of its own", (file) => {
    expect(viewSource(file)).not.toContain("scrollTop");
  });
});
