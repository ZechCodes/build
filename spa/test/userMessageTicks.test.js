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

describe("user message ticks", () => {
  it("places one centered gutter button beside each reader message", () => {
    const scroller = mount();
    const ticks = scroller.querySelectorAll(".thread-user-tick");
    expect(ticks).toHaveLength(2);
    expect([...ticks].every((tick) => tick.closest(".thread-message.user"))).toBe(true);
    expect(ticks[0].getAttribute("aria-label")).toContain("Jump to your message from Sep 22, 2026");
    expect(styles).toMatch(/\.thread-user-tick \{[^}]*left:0; top:50%; transform:translateY\(-50%\)/);
    expect(styles).toMatch(/\.thread-message\.user \{ width:100%; \}/);
  });

  it("highlights only the last user message at or above the scroll boundary", () => {
    const scroller = mount();
    const rows = scroller.querySelectorAll(".thread-message.user");
    let positions = [90, 180];
    scroller.getBoundingClientRect = () => ({ top: 100 });
    rows.forEach((row, index) => { row.getBoundingClientRect = () => ({ top: positions[index] }); });

    syncUserMessageTicks(scroller);
    expect([...scroller.querySelectorAll(".thread-user-tick.active")]).toEqual([rows[0].querySelector("button")]);

    positions = [20, 120];
    syncUserMessageTicks(scroller);
    expect([...scroller.querySelectorAll(".thread-user-tick.active")]).toEqual([rows[1].querySelector("button")]);
    expect(rows[1].querySelector("button").getAttribute("aria-current")).toBe("location");
    expect(rows[0].querySelector("button").hasAttribute("aria-current")).toBe(false);

    positions = [121, 200];
    syncUserMessageTicks(scroller);
    expect(scroller.querySelector(".thread-user-tick.active")).toBeNull();
  });

  it("jumps to the clicked message through its keyboard-operable button", () => {
    const scroller = mount();
    const row = scroller.querySelectorAll(".thread-message.user")[1];
    row.scrollIntoView = vi.fn();
    const tick = row.querySelector(".thread-user-tick");
    expect(tick.tagName).toBe("BUTTON");
    expect(tick.type).toBe("button");
    expect(jumpToUserMessage({ target: tick.querySelector("span") })).toBe(true);
    expect(row.scrollIntoView).toHaveBeenCalledWith({ behavior: "smooth", block: "start" });
    expect(jumpToUserMessage({ target: scroller })).toBe(false);
  });
});
