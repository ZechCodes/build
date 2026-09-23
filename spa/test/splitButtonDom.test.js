// @vitest-environment jsdom
// DOM wiring for mountSplitButton: single-flight while run() is awaiting —
// primary + caret disabled, menu invokes ignored — and the shared-flight form
// (a remount mid-flight keeps the latch, so no concurrent destructive RPCs).

import { describe, it, expect, vi } from "vitest";
import { waitFor } from "./waitFor.js";
import { mountSplitButton, createSingleFlight } from "../src/core/splitButton.js";

const OPTIONS = [
  { id: "merge_prune", label: "Merge", menuLabel: "Merge & clean up", description: "d", busyLabel: "merging…" },
  { id: "commit", menuLabel: "Commit", description: "d", busyLabel: "committing…" },
];

function pendingRun() {
  const calls = [];
  let settle;
  const gate = new Promise((resolve) => (settle = resolve));
  const run = (optionId) => {
    calls.push(optionId);
    return gate;
  };
  return { run, calls, settle };
}

function mount(runSpec, flight) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  mountSplitButton(container, { options: OPTIONS, run: runSpec.run, flight });
  return {
    container,
    primary: container.querySelector(".btn.primary:not(.caret)"),
    caret: container.querySelector(".caret"),
    menu: container.querySelector(".splitmenu"),
  };
}

describe("mountSplitButton in-flight guard (DOM)", () => {
  it("disables primary + caret and shows the busy label while run() is pending", async () => {
    const spec = pendingRun();
    const flight = createSingleFlight();
    const { primary, caret } = mount(spec, flight);
    primary.click();
    expect(primary.disabled).toBe(true);
    expect(caret.disabled).toBe(true);
    expect(primary.textContent).toBe("merging…");
    spec.settle();
    await waitFor(() => expect(flight.active()).toBe(false));
    expect(spec.calls).toEqual(["merge_prune"]);
  });

  it("ignores menu invokes while in flight (single flight)", async () => {
    const spec = pendingRun();
    const flight = createSingleFlight();
    const { primary, menu } = mount(spec, flight);
    primary.click();
    menu.querySelector('[data-action="commit"]').click();
    expect(spec.calls).toEqual(["merge_prune"]);
    spec.settle();
    await waitFor(() => expect(flight.active()).toBe(false));
  });

  it("a rejection restores label + enabled so the user can retry", async () => {
    const calls = [];
    const run = (optionId) => {
      calls.push(optionId);
      return Promise.reject(new Error("boom"));
    };
    const container = document.createElement("div");
    document.body.appendChild(container);
    mountSplitButton(container, { options: OPTIONS, run });
    const primary = container.querySelector(".btn.primary:not(.caret)");
    primary.click();
    await waitFor(() => expect(primary.disabled).toBe(false));
    expect(primary.disabled).toBe(false);
    expect(primary.textContent).toBe("Merge");
    primary.click();
    await waitFor(() => expect(primary.disabled).toBe(false));
    expect(calls).toEqual(["merge_prune", "merge_prune"]);
  });

  // A real pointer presses before it clicks. The outside-press watch has to be
  // armed in the same event cycle as the opening click (a deferred one loses
  // the race), so it must treat the split button's own pointers as inside.
  describe("the menu the caret opens", () => {
    const press = (target) => target.dispatchEvent(new Event("pointerdown", { bubbles: true }));

    it("stays open through the pointer sequence that opened it", () => {
      const { caret, menu } = mount(pendingRun());
      press(caret);
      caret.click();
      expect(menu.hidden).toBe(false);
    });

    it("stays open while the pointer travels onto an item", () => {
      const { caret, menu } = mount(pendingRun());
      press(caret);
      caret.click();
      press(menu.querySelector('[data-action="commit"]'));
      expect(menu.hidden).toBe(false);
    });

    it("closes on a press outside it", () => {
      const { caret, menu } = mount(pendingRun());
      press(caret);
      caret.click();
      press(document.body);
      expect(menu.hidden).toBe(true);
    });

    it("stops watching for outside presses once the caret shuts it again", () => {
      const { caret, menu } = mount(pendingRun());
      caret.click();
      caret.click();
      expect(menu.hidden).toBe(true);
      caret.click();
      press(document.body);
      expect(menu.hidden).toBe(true);
      // One open leaves exactly one watch behind: reopening still closes once.
      caret.click();
      expect(menu.hidden).toBe(false);
    });

    it("places an upward menu in viewport coordinates under a filtered containing block", () => {
      const { container, caret, menu } = mount(pendingRun());
      const scrolling = document.createElement("div");
      scrolling.style.overflowY = "hidden";
      scrolling.append(container);
      document.body.append(scrolling);
      // Low enough in the viewport that the menu does not fit under the button,
      // which is the whole of what sends it upward.
      container.querySelector(".splitbtn").getBoundingClientRect = () => ({
        left: 268, right: 300, top: 700, bottom: 732, width: 32, height: 32,
      });
      let menuHeight = 96;
      Object.defineProperties(menu, {
        offsetWidth: { configurable: true, value: 180 },
        offsetHeight: { configurable: true, get: () => menuHeight },
      });
      const containingBlock = { left: 100, top: 50, height: 700, scale: 0.8 };
      menu.getBoundingClientRect = () => {
        const height = menuHeight * containingBlock.scale;
        const bottomInset = Number.parseFloat(menu.style.bottom);
        const bottom = Number.isFinite(bottomInset)
          ? containingBlock.top + (containingBlock.height - bottomInset) * containingBlock.scale
          : containingBlock.top + height;
        return {
          left: containingBlock.left + (Number.parseFloat(menu.style.left) || 0) * containingBlock.scale,
          right: containingBlock.left + (Number.parseFloat(menu.style.left) || 0) * containingBlock.scale + 180 * containingBlock.scale,
          top: bottom - height,
          bottom,
          width: 180 * containingBlock.scale,
          height,
        };
      };

      caret.click();

      const placed = menu.getBoundingClientRect();
      expect(placed.left + placed.width).toBeCloseTo(300);
      expect(placed.top + placed.height).toBeCloseTo(694);

      menuHeight = 48;
      const shrinking = menu.getBoundingClientRect();
      expect(shrinking.bottom).toBeCloseTo(694);
      expect(shrinking.top).toBeGreaterThan(placed.top);
    });

    it("places a downward menu in viewport coordinates under the same filtered block", () => {
      // The conversation head's ⋮: a button near the top of a glass header,
      // whose backdrop-filter makes `position:fixed` resolve from the header.
      const { container, caret, menu } = mount(pendingRun());
      const scrolling = document.createElement("div");
      scrolling.style.overflowY = "hidden";
      scrolling.append(container);
      document.body.append(scrolling);
      container.querySelector(".splitbtn").getBoundingClientRect = () => ({
        left: 268, right: 300, top: 24, bottom: 56, width: 32, height: 32,
      });
      Object.defineProperties(menu, {
        offsetWidth: { configurable: true, value: 180 },
        offsetHeight: { configurable: true, value: 96 },
      });
      const containingBlock = { left: 100, top: 50, scale: 0.8 };
      menu.getBoundingClientRect = () => {
        const topInset = Number.parseFloat(menu.style.top) || 0;
        return {
          left: containingBlock.left + (Number.parseFloat(menu.style.left) || 0) * containingBlock.scale,
          top: containingBlock.top + topInset * containingBlock.scale,
          bottom: containingBlock.top + (topInset + 96) * containingBlock.scale,
          right: containingBlock.left + ((Number.parseFloat(menu.style.left) || 0) + 180) * containingBlock.scale,
          width: 180 * containingBlock.scale,
          height: 96 * containingBlock.scale,
        };
      };

      caret.click();

      const placed = menu.getBoundingClientRect();
      expect(menu.style.bottom).toBe("auto");
      expect(placed.top).toBeCloseTo(62);
      expect(placed.left + placed.width).toBeCloseTo(300);
      expect(placed.bottom).toBeLessThanOrEqual(window.innerHeight);
    });

    it("leaves ordinary viewport-fixed placement unchanged", () => {
      const { container, caret, menu } = mount(pendingRun());
      const scrolling = document.createElement("div");
      scrolling.style.overflowY = "auto";
      scrolling.append(container);
      document.body.append(scrolling);
      container.querySelector(".splitbtn").getBoundingClientRect = () => ({
        left: 768, right: 800, top: 20, bottom: 52, width: 32, height: 32,
      });
      Object.defineProperties(menu, {
        offsetWidth: { configurable: true, value: 180 },
        offsetHeight: { configurable: true, value: 96 },
      });
      menu.getBoundingClientRect = () => ({
        left: Number.parseFloat(menu.style.left) || 0,
        top: Number.parseFloat(menu.style.top) || 0,
        width: 180,
        height: 96,
      });

      caret.click();

      expect(menu.getBoundingClientRect()).toMatchObject({ left: 620, top: 58 });
    });
  });

  // Every caller of this component sits under a poll. A tick that would mount
  // the very same button must not take the menu the user just opened.
  describe("a remount under an open menu", () => {
    it("leaves the open menu standing when nothing about the button changed", () => {
      const spec = pendingRun();
      const { container, caret, menu } = mount(spec);
      caret.click();
      expect(menu.hidden).toBe(false);

      mountSplitButton(container, { options: OPTIONS, run: spec.run }); // the poll tick

      expect(container.querySelector(".splitmenu"), "the menu element was replaced").toBe(menu);
      expect(menu.hidden).toBe(false);
      // And the item the user was reaching for still runs.
      menu.querySelector('[data-action="commit"]').click();
      expect(spec.calls).toEqual(["commit"]);
    });

    it("rebuilds anyway when the options themselves changed", () => {
      const spec = pendingRun();
      const { container, caret, menu } = mount(spec);
      caret.click();
      expect(menu.hidden).toBe(false);

      mountSplitButton(container, {
        options: [OPTIONS[0], { id: "push", menuLabel: "Push", description: "d", busyLabel: "pushing…" }],
        run: spec.run,
      });

      expect(container.querySelector(".splitmenu")).not.toBe(menu);
      expect(container.textContent).toContain("Push");
    });

    it("still repaints once the menu is shut again", () => {
      const spec = pendingRun();
      const { container, caret } = mount(spec);
      caret.click();
      caret.click(); // shut
      const before = container.querySelector(".splitmenu");
      mountSplitButton(container, { options: OPTIONS, run: spec.run });
      expect(container.querySelector(".splitmenu")).not.toBe(before);
    });
  });

  it("a shared external flight blocks invokes on a remounted button mid-flight", async () => {
    const flight = createSingleFlight();
    const first = pendingRun();
    const a = mount(first, flight);
    a.primary.click();
    expect(first.calls).toEqual(["merge_prune"]);

    // A poll tick remounts the button while the RPC is still in flight: the
    // fresh button shares the latch, so a click dispatches nothing.
    const second = pendingRun();
    const b = mount(second, flight);
    b.primary.click();
    b.menu.querySelector('[data-action="commit"]').click();
    expect(second.calls).toEqual([]);

    // Once the original flight settles, the latch re-arms.
    first.settle();
    await waitFor(() => expect(flight.active()).toBe(false));
    b.primary.click();
    expect(second.calls).toEqual(["merge_prune"]);
  });
});
