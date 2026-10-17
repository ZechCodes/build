// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
import { bubbleHtml } from "../src/core/agentRail.js";
import { createRailBubbleTooltip, positionRailBubbleTooltip } from "../src/core/railBubbleTooltip.js";

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const bubble = (over = {}) => ({
  type: "agent", id: "ag-1", title: "Rail helper", pattern: 1, active: false,
  unread: 0, working: false, tooltip: {
    name: "Rail helper", work: "Fix <the> rail", provider: "Claude Code",
    model: "Claude Opus 5", status: "idle", workingTime: null,
  }, ...over,
});

describe("conversation bubble tooltip", () => {
  it("replaces native titles on agent, project, and ghost conversation bubbles", () => {
    for (const type of ["agent", "project", "ghost"]) {
      const host = document.createElement("div");
      host.innerHTML = bubbleHtml(bubble({ type }));
      expect(host.firstElementChild.hasAttribute("title")).toBe(false);
      expect(host.firstElementChild.dataset.railTooltip).toBeTruthy();
    }
    const legacy = document.createElement("div");
    legacy.innerHTML = bubbleHtml(bubble({ tooltip: null }));
    expect(legacy.firstElementChild.hasAttribute("title")).toBe(false);
  });

  it("shows cached name, work, current-turn runtime and harness/model on hover and focus", () => {
    vi.useFakeTimers();
    const button = document.createElement("button");
    button.dataset.railTooltip = JSON.stringify({
      ...bubble().tooltip, status: "working", workingTime: { since: "2026-10-17T12:00:00Z" }, watch: "Not watching",
    });
    document.body.append(button);
    vi.setSystemTime(new Date("2026-10-17T12:01:25Z"));
    const tip = createRailBubbleTooltip();
    tip.wire(button);
    button.dispatchEvent(new Event("pointerenter"));
    expect(document.querySelector(".rail-bubble-tooltip").textContent).toContain("Rail helper");
    expect(document.querySelector(".rail-bubble-tooltip").textContent).toContain("Fix <the> rail");
    expect(document.querySelector(".rail-bubble-tooltip").textContent).toContain("1m 25s");
    expect(document.querySelector(".rail-bubble-tooltip").textContent).toContain("Claude Code · Claude Opus 5");
    expect(document.querySelector(".rail-bubble-tooltip").textContent).toContain("Not watching");
    button.dispatchEvent(new Event("pointerleave"));
    expect(document.querySelector(".rail-bubble-tooltip")).toBeNull();
    button.focus();
    expect(document.querySelector(".rail-bubble-tooltip")).toBeTruthy();
    tip.dispose();
    expect(document.querySelector(".rail-bubble-tooltip")).toBeNull();
  });

  it("clamps the portal inside narrow viewport edges", () => {
    vi.stubGlobal("innerWidth", 240);
    vi.stubGlobal("innerHeight", 160);
    const tip = document.createElement("div");
    tip.getBoundingClientRect = () => ({ width: 150, height: 80 });
    const button = document.createElement("button");
    button.getBoundingClientRect = () => ({ left: 220, right: 252, top: 135, bottom: 167, height: 32 });
    positionRailBubbleTooltip(tip, button);
    expect(Number.parseFloat(tip.style.left)).toBeGreaterThanOrEqual(8);
    expect(Number.parseFloat(tip.style.left)).toBeLessThanOrEqual(82);
    expect(Number.parseFloat(tip.style.top)).toBeGreaterThanOrEqual(8);
    expect(Number.parseFloat(tip.style.top)).toBeLessThanOrEqual(72);
  });

  it("restores the tooltip relation after a retained button repaint and closes on Escape", () => {
    const button = document.createElement("button");
    button.dataset.railTooltip = JSON.stringify(bubble().tooltip);
    document.body.append(button);
    const controller = createRailBubbleTooltip();
    controller.wire(button);
    button.focus();
    const describedBy = button.getAttribute("aria-describedby");
    expect(describedBy).toBeTruthy();
    button.removeAttribute("aria-describedby");
    button.dataset.railTooltip = JSON.stringify({ ...bubble().tooltip, name: "Updated agent" });
    controller.refresh();
    expect(button.getAttribute("aria-describedby")).toBe(describedBy);
    expect(document.querySelector(".rail-bubble-tooltip-name").textContent).toBe("Updated agent");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(document.querySelector(".rail-bubble-tooltip")).toBeNull();
    controller.dispose();
  });

  it("does not show a tooltip from a touch-origin focus and gives concurrent rails unique IDs", () => {
    const button = document.createElement("button");
    button.dataset.railTooltip = JSON.stringify(bubble().tooltip);
    const other = button.cloneNode();
    document.body.append(button, other);
    const first = createRailBubbleTooltip();
    const second = createRailBubbleTooltip();
    first.wire(button);
    second.wire(other);
    button.dispatchEvent(new PointerEvent("pointerdown", { pointerType: "touch" }));
    button.focus();
    expect(document.querySelector(".rail-bubble-tooltip")).toBeNull();
    other.focus();
    const secondId = other.getAttribute("aria-describedby");
    button.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    button.focus();
    expect(button.getAttribute("aria-describedby")).not.toBe(secondId);
    first.dispose();
    second.dispose();
  });
});
