// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { compactionMenuGroup } from "../src/core/conversationCompaction.js";
import { groupedMenuButtonMarkup, mountSplitMenu } from "../src/core/splitButton.js";
import { motionBeat } from "./motionRecorder.js";

const keydown = (target, key) => target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));

function mount(agent = { max_context_tokens: null, compact_at_tokens: 200000 }) {
  const host = document.createElement("div");
  host.innerHTML = groupedMenuButtonMarkup("⋮", [
    { id: "detail", label: "Detail", options: [{ id: "detail:all", label: "All", description: "Everything", selected: true }] },
    compactionMenuGroup(agent),
  ], { title: "Conversation menu", icon: true });
  document.body.appendChild(host);
  const onChoose = vi.fn();
  const menu = mountSplitMenu(host, { onChoose });
  return { host, onChoose, menu, caret: host.querySelector(".caret"), slider: host.querySelector('[role="slider"]') };
}

afterEach(() => { document.body.innerHTML = ""; });

describe("the compaction slider in a split menu", () => {
  it("shows only the cached stop's word and description, with discrete accessible values", () => {
    const { host, slider } = mount();
    expect(slider).not.toBeNull();
    expect(slider.type).toBe("range");
    expect(slider.tabIndex).toBe(0);
    expect([slider.min, slider.max, slider.step, slider.value]).toEqual(["0", "4", "1", "0"]);
    expect(slider.getAttribute("aria-label")).toBe("Compact at");
    expect(slider.getAttribute("aria-valuetext")).toBe("Default (200k)");
    const group = host.querySelector('[data-group="compact"]');
    expect(group.querySelectorAll(".mt")).toHaveLength(1);
    expect(group.querySelector(".mt").textContent).toBe("Default (200k)");
    expect(group.querySelector(".md").textContent).toBe("This device's setting");
    expect(group.querySelectorAll('[role="menuitemradio"]')).toHaveLength(0);
    expect(document.getElementById(slider.getAttribute("aria-describedby")).textContent).toBe("This device's setting");
  });

  it("previews only whole stops while dragging and commits once on change", () => {
    const { host, slider, caret, onChoose } = mount();
    caret.click();
    slider.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    slider.value = "2";
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    expect(onChoose).not.toHaveBeenCalled();
    expect(host.querySelector(".splitmenu").hidden).toBe(false);
    expect(host.querySelector('[data-group="compact"] .mt').textContent).toBe("200k");
    expect(slider.getAttribute("aria-valuetext")).toBe("200k");
    slider.value = "4";
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    slider.dispatchEvent(new Event("change", { bubbles: true }));
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:off");
    expect(document.activeElement).toBe(caret);
  });

  it("keeps a custom cached limit as its own stop ahead of Off", () => {
    const { slider } = mount({ max_context_tokens: 250000, compact_at_tokens: 250000 });
    expect([slider.max, slider.value]).toEqual(["5", "4"]);
    expect(slider.getAttribute("aria-valuetext")).toBe("Custom (250k)");
    expect(slider.dataset.action).toBe("compact:250000");
  });

  it("reaches the thumb from the menu, leaves adjustment keys to the native range, and escapes", async () => {
    const { caret, slider, host, onChoose } = mount();
    caret.focus();
    keydown(caret, "ArrowUp");
    expect(document.activeElement).toBe(slider);
    for (const key of ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"]) {
      expect(keydown(slider, key)).toBe(true);
      expect(document.activeElement).toBe(slider);
    }
    expect(onChoose).not.toHaveBeenCalled();
    keydown(slider, "Escape");
    await motionBeat();
    expect(host.querySelector(".splitmenu").hidden).toBe(true);
    expect(document.activeElement).toBe(caret);
  });

  it("can commit the standing stop with Enter", () => {
    const { caret, slider, onChoose } = mount();
    keydown(caret, "ArrowUp");
    keydown(slider, "Enter");
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:default");
  });

  it("restores the cached value when a preview is cancelled or a write is refused", async () => {
    const { caret, slider, host, onChoose } = mount();
    for (const event of ["Escape", "change"]) {
      caret.click();
      slider.focus();
      slider.value = "3";
      slider.dispatchEvent(new Event("input", { bubbles: true }));
      if (event === "Escape") keydown(slider, event);
      else slider.dispatchEvent(new Event(event, { bubbles: true }));
      await motionBeat();
      caret.click();
      expect(slider.getAttribute("aria-valuetext")).toBe("Default (200k)");
      expect(slider.value).toBe("0");
      keydown(slider, "Escape");
      await motionBeat();
    }
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:300000");
    expect(host.querySelector('[data-group="compact"] .mt').textContent).toBe("Default (200k)");
  });
});
