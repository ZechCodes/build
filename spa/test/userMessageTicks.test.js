// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { jumpToUserMessage, syncUserMessageTicks, threadHtml } from "../src/core/thread.js";

const styles = readFileSync(resolve("src/styles.css"), "utf8");

function mount() {
  document.body.innerHTML = `<div id="scroller" style="scroll-padding-top:20px">${threadHtml({ items: [
    { type: "message", data: { sequence: 1, role: "user", body: "First", created_at: "2026-09-22T12:00:00Z" } },
    { type: "message", data: { sequence: 2, role: "agent", body: "Reply" } },
    { type: "message", data: { sequence: 3, role: "user", body: "From another agent", from_agent: { id: "a-2" } } },
    { type: "message", data: { sequence: 4, role: "user", body: "Second", created_at: "2026-09-22T13:00:00Z" } },
  ] })}</div>`;
  return document.querySelector("#scroller");
}

describe("grouped user message navigator", () => {
  it("groups horizontal lines in one sticky gutter column, excluding another agent's words", () => {
    const scroller = mount();
    const navigator = scroller.querySelectorAll(".thread-user-nav");
    const ticks = scroller.querySelectorAll(".thread-user-tick");
    expect(navigator).toHaveLength(1);
    expect(ticks).toHaveLength(2);
    expect([...ticks].map((tick) => tick.dataset.userTickIndex)).toEqual(["0", "1"]);
    expect([...ticks].every((tick) => tick.closest(".thread-user-nav") === navigator[0])).toBe(true);
    expect(scroller.querySelector(".thread-message .thread-user-tick")).toBeNull();
    expect(ticks[0].getAttribute("aria-label")).toContain("Jump to your message from Sep 22, 2026");
    expect(styles).toMatch(/\.thread-user-nav \{ position:sticky; top:50%/);
    expect(styles).toMatch(/\.thread-user-tick span \{[^}]*width:10px; height:2px/);
  });

  it("widens the nearest previous line and any hovered line, leaving the others narrow", () => {
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
    expect(styles).toMatch(/\.thread-user-tick:is\(:hover, \.active\) span \{ width:20px; \}/);
  });

  it("jumps from a keyboard-operable grouped line to its message", () => {
    const scroller = mount();
    const row = scroller.querySelectorAll(".thread-message.user")[1];
    row.scrollIntoView = vi.fn();
    const tick = scroller.querySelectorAll(".thread-user-tick")[1];
    expect(tick.tagName).toBe("BUTTON");
    expect(tick.type).toBe("button");
    expect(jumpToUserMessage({ target: tick.querySelector("span") })).toBe(true);
    expect(row.scrollIntoView).toHaveBeenCalledWith({ behavior: "smooth", block: "start" });
    expect(jumpToUserMessage({ target: scroller })).toBe(false);
  });
});
