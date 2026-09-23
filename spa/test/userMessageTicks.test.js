// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it, vi } from "vitest";
import {
  jumpToUserMessage,
  scheduleUserMessageTickSync,
  syncUserMessageTicks,
  threadHtml,
} from "../src/core/thread.js";

const styles = readFileSync(resolve("src/styles.css"), "utf8");
const railStyles = readFileSync(resolve("src/styles/shell.css"), "utf8");

function mount() {
  document.body.innerHTML = `<div id="scroller" style="scroll-padding-top:20px">${threadHtml({ items: [
    { type: "message", data: { sequence: 1, role: "user", body: "First", created_at: "2026-09-22T12:00:00Z" } },
    { type: "message", data: { sequence: 2, role: "agent", body: "Reply" } },
    { type: "message", data: { sequence: 3, role: "user", body: "From another agent", from_agent: { id: "a-2" } } },
    { type: "message", data: { sequence: 4, role: "user", body: "Second", created_at: "2026-09-22T13:00:00Z" } },
  ] })}</div>`;
  return document.querySelector("#scroller");
}

function mountMany(count) {
  document.body.innerHTML = `<div id="scroller" style="scroll-padding-top:20px">${threadHtml({ items: Array.from({ length: count }, (_, index) => ({
    type: "message", data: { sequence: index + 1, role: "user", body: `Message ${index + 1}` },
  })) })}</div>`;
  const scroller = document.querySelector("#scroller");
  scroller.scrollTo = vi.fn();
  const rows = [...scroller.querySelectorAll(".thread-items > .thread-message.user")];
  scroller.getBoundingClientRect = () => ({ top: 100 });
  let active = -1;
  rows.forEach((row, index) => {
    row.getBoundingClientRect = () => ({ top: 120 + (index - active) * 80 });
  });
  return {
    scroller, rows,
    setActive(index) {
      active = index;
      rows.forEach((row, rowIndex) => {
        row.getBoundingClientRect = () => ({ top: 120 + (rowIndex - active) * 80 });
      });
      syncUserMessageTicks(scroller);
    },
  };
}

const visibleIndexes = (scroller) => [...scroller.querySelectorAll(".thread-user-tick")]
  .map((tick) => Number(tick.dataset.userTickIndex));

describe("grouped user message navigator", () => {
  it("groups pill ticks in the existing gutter without reserving message width", () => {
    const scroller = mount();
    const navigator = scroller.querySelectorAll(".thread-user-nav");
    const ticks = scroller.querySelectorAll(".thread-user-tick");
    expect(navigator).toHaveLength(1);
    expect(ticks).toHaveLength(2);
    expect([...ticks].map((tick) => tick.dataset.userTickIndex)).toEqual(["0", "1"]);
    expect([...ticks].every((tick) => tick.closest(".thread-user-nav") === navigator[0])).toBe(true);
    expect(scroller.querySelector(".thread-message .thread-user-tick")).toBeNull();
    expect(ticks[0].getAttribute("aria-label")).toContain("Jump to your message from Sep 22, 2026");
    expect(styles).toMatch(/\.thread-user-nav \{ position:sticky; top:50%;[^}]*width:0; height:0/);
    expect(styles).toMatch(/\.thread-user-nav-list \{ position:absolute; top:0; left:-20px/);
    expect(styles).toMatch(/\.thread-timeline \{ --thread-gap:20px; gap:var\(--thread-gap\); padding:20px 12px; \}/);
    expect(railStyles).toMatch(/\.rail-body \.thread-timeline \{[^}]*padding:4px 12px 16px; \}/);
    expect(styles).toMatch(/\.thread-user-tick span \{[^}]*width:10px; height:3px; border-radius:999px/);
    expect(styles).toMatch(/transition:width 200ms ease-out, background-color 200ms ease-out/);
  });

  it("widens exactly one nearest previous pill and any hovered pill to twice the resting width", () => {
    const scroller = mount();
    const rows = scroller.querySelectorAll(".thread-message.user");
    const ticks = scroller.querySelectorAll(".thread-user-tick");
    let positions = [90, 180];
    scroller.getBoundingClientRect = () => ({ top: 100 });
    rows.forEach((row, index) => { row.getBoundingClientRect = () => ({ top: positions[index] }); });

    syncUserMessageTicks(scroller);
    expect([...scroller.querySelectorAll(".thread-user-tick.active")]).toEqual([ticks[0]]);

    positions = [20, 120];
    syncUserMessageTicks(scroller);
    expect([...scroller.querySelectorAll(".thread-user-tick.active")]).toEqual([ticks[1]]);
    expect(ticks[1].getAttribute("aria-current")).toBe("location");
    expect(ticks[0].hasAttribute("aria-current")).toBe(false);

    positions = [121, 200];
    syncUserMessageTicks(scroller);
    expect(scroller.querySelector(".thread-user-tick.active")).toBeNull();
    expect(styles).toMatch(/\.thread-user-tick:is\(:hover, \.active\) span \{ width:20px;/);
    expect(styles).toMatch(/background-color:color-mix\(in srgb, var\(--dim\) 55%, var\(--accent\)\)/);
  });

  it("coalesces scroll updates into one animation frame and reads the latest position", () => {
    const scroller = mount();
    const rows = scroller.querySelectorAll(".thread-message.user");
    const ticks = scroller.querySelectorAll(".thread-user-tick");
    let positions = [90, 180];
    scroller.getBoundingClientRect = () => ({ top: 100 });
    rows.forEach((row, index) => { row.getBoundingClientRect = () => ({ top: positions[index] }); });
    const frames = [];
    const originalFrame = window.requestAnimationFrame;
    window.requestAnimationFrame = vi.fn((callback) => { frames.push(callback); return frames.length; });
    try {
      scheduleUserMessageTickSync(scroller);
      scheduleUserMessageTickSync(scroller);
      positions = [20, 120];
      scheduleUserMessageTickSync(scroller);
      expect(frames).toHaveLength(1);
      expect(scroller.querySelector(".thread-user-tick.active")).toBeNull();
      frames.shift()();
      expect([...scroller.querySelectorAll(".thread-user-tick.active")]).toEqual([ticks[1]]);
      scheduleUserMessageTickSync(scroller);
      expect(frames).toHaveLength(1);
    } finally {
      window.requestAnimationFrame = originalFrame;
    }
  });

  it("keeps the current pill visible when the pinned group has its own scroll", () => {
    const scroller = mount();
    const list = scroller.querySelector(".thread-user-nav-list");
    const ticks = list.querySelectorAll(".thread-user-tick");
    const rows = scroller.querySelectorAll(".thread-message.user");
    scroller.getBoundingClientRect = () => ({ top: 100 });
    rows.forEach((row) => { row.getBoundingClientRect = () => ({ top: 0 }); });
    Object.defineProperty(list, "clientHeight", { value: 24 });
    Object.defineProperty(ticks[1], "offsetTop", { value: 24 });
    Object.defineProperty(ticks[1], "offsetHeight", { value: 24 });

    syncUserMessageTicks(scroller);
    expect(ticks[1].classList.contains("active")).toBe(true);
    expect(list.scrollTop).toBe(24);
  });

  it("jumps from a keyboard-operable grouped line to its message", () => {
    const scroller = mount();
    const row = scroller.querySelectorAll(".thread-message.user")[1];
    scroller.scrollTo = vi.fn();
    const tick = scroller.querySelectorAll(".thread-user-tick")[1];
    expect(tick.tagName).toBe("BUTTON");
    expect(tick.type).toBe("button");
    expect(jumpToUserMessage({ target: tick.querySelector("span") })).toBe(true);
    expect(scroller.scrollTo).toHaveBeenCalledWith({ top: 0, behavior: "smooth" });
    expect(jumpToUserMessage({ target: scroller })).toBe(false);
  });

  it("slides a contiguous twelve-tick window down and back up, keeping the nearest pill", () => {
    const { scroller, setActive } = mountMany(24);
    expect(visibleIndexes(scroller)).toEqual(Array.from({ length: 12 }, (_, index) => index));
    expect(scroller.querySelector(".thread-user-tick.active")).toBeNull();

    setActive(10);
    expect(visibleIndexes(scroller)).toEqual(Array.from({ length: 12 }, (_, index) => index + 5));
    expect(scroller.querySelectorAll(".thread-user-tick.active")).toHaveLength(1);
    expect(scroller.querySelector(".thread-user-tick.active").dataset.userTickIndex).toBe("10");

    setActive(18);
    expect(visibleIndexes(scroller)).toEqual(Array.from({ length: 12 }, (_, index) => index + 12));
    setActive(7);
    expect(visibleIndexes(scroller)).toEqual(Array.from({ length: 12 }, (_, index) => index + 2));
    expect(scroller.querySelector(".thread-user-tick.active").dataset.userTickIndex).toBe("7");
  });

  it("pins to the first and last twelve, and leaves the top state without an active pill", () => {
    const { scroller, setActive } = mountMany(20);
    setActive(-1);
    expect(visibleIndexes(scroller)).toEqual(Array.from({ length: 12 }, (_, index) => index));
    expect(scroller.querySelector(".thread-user-tick.active")).toBeNull();
    setActive(0);
    expect(visibleIndexes(scroller)[0]).toBe(0);
    setActive(1);
    expect(visibleIndexes(scroller)[0]).toBe(0);
    setActive(18);
    expect(visibleIndexes(scroller)).toEqual(Array.from({ length: 12 }, (_, index) => index + 8));
    setActive(19);
    expect(visibleIndexes(scroller).at(-1)).toBe(19);

    const short = mountMany(12);
    short.setActive(11);
    expect(visibleIndexes(short.scroller)).toEqual(Array.from({ length: 12 }, (_, index) => index));
  });

  it("jumps from a window-edge pill to the matching global message", () => {
    const { scroller, rows, setActive } = mountMany(20);
    setActive(19);
    const edge = scroller.querySelector('.thread-user-tick[data-user-tick-index="8"]');
    expect(edge.type).toBe("button");
    expect(jumpToUserMessage({ target: edge.querySelector("span") })).toBe(true);
    expect(scroller.scrollTo).toHaveBeenCalledWith({ top: -860, behavior: "smooth" });
  });
});
