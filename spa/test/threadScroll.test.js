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

  // ---- reading back into history -------------------------------------------
  //
  // A long conversation opens on a page of it, so the top of the scroller is a
  // floor rather than the start. Lifting it puts content ABOVE the reader,
  // which is the one direction the pinning above does not cover.

  it("holds the reader on the message they were reading when older items land above it", () => {
    let height = 1000;
    const body = document.createElement("div");
    Object.defineProperty(body, "scrollHeight", { get: () => height, configurable: true });
    Object.defineProperty(body, "clientHeight", { get: () => 300, configurable: true });
    body.scrollTop = 0;
    document.body.appendChild(body);
    paintThreadKeepingPlace(body, paintThread(body)); // opens: bottom
    body.scrollTop = 0; // the reader reads all the way back to the top

    // The paint itself is what makes the timeline taller: 600px of history
    // above everything that was already there.
    const paintWithHistoryOnTop = () => {
      height = 1600;
      paintThread(body)();
    };
    paintThreadKeepingPlace(body, paintWithHistoryOnTop, { olderItemsPrepended: true });

    expect(body.scrollTop).toBe(600);
  });

  it("never takes a reader asking for history to the bottom instead", () => {
    let height = 1000;
    const body = document.createElement("div");
    Object.defineProperty(body, "scrollHeight", { get: () => height, configurable: true });
    Object.defineProperty(body, "clientHeight", { get: () => 300, configurable: true });
    body.scrollTop = 700; // at the bottom, where a short thread opens
    document.body.appendChild(body);
    paintThreadKeepingPlace(body, paintThread(body));
    body.scrollTop = 0;

    paintThreadKeepingPlace(body, () => {
      height = 1200;
      paintThread(body)();
    }, { olderItemsPrepended: true });

    expect(body.scrollTop).toBe(200);
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

const sourceOf = (file) => readFileSync(resolve("src", file), "utf8");

// The conversation lives in the agent rail now; no view paints a thread of
// its own — a task's conversation belongs to its agent, and an agent's
// conversation lives in the rail beside every surface.
const SURFACES_WITH_A_CONVERSATION = ["core/agentRail.js"];

describe("every surface's conversation scrolls the same way", () => {
  it.each(SURFACES_WITH_A_CONVERSATION)("%s wraps every thread paint in the shared helper", (file) => {
    const source = sourceOf(file);
    const bodyLines = source.split("\n").filter((line) => !/^import\b/.test(line.trim()));
    const threadPaints = bodyLines.join("\n").match(/paintThreadEntries\(/g) || [];
    const wrapped = bodyLines.join("\n").match(/paintThreadKeepingPlace\(/g) || [];
    expect(threadPaints.length).toBeGreaterThan(0);
    expect(wrapped).toHaveLength(threadPaints.length);
  });

  // Reading where the reader is standing is fair; moving them is the helper's.
  it.each(SURFACES_WITH_A_CONVERSATION)("%s never moves the scroller itself", (file) => {
    expect(sourceOf(file)).not.toMatch(/scrollTop\s*=/);
  });
});
