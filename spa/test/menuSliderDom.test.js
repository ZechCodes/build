// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { compactionMenuGroup } from "../src/core/conversationCompaction.js";
import { groupedMenuButtonMarkup, mountMenuIfChanged, mountSplitMenu } from "../src/core/splitButton.js";
import { motionBeat } from "./motionRecorder.js";

const keydown = (target, key) => target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
const pointer = (target, type) => target.dispatchEvent(Object.assign(new Event(type, { bubbles: true }), { pointerId: 1 }));
const preview = (slider, index) => {
  slider.value = String(index);
  slider.dispatchEvent(new Event("input", { bubbles: true }));
  slider.dispatchEvent(new Event("change", { bubbles: true }));
};

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
  it("shows only the cached stop's word and ticks, with discrete accessible values", () => {
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
    expect(group.querySelector(".md")).toBeNull();
    expect(group.querySelectorAll(".menu-slider-stops span")).toHaveLength(5);
    expect(group.querySelectorAll('[role="menuitemradio"]')).toHaveLength(0);
    expect(slider.hasAttribute("aria-describedby")).toBe(false);
  });

  it("previews only whole stops while dragging and commits once on release without closing", async () => {
    const { host, slider, caret, onChoose } = mount();
    caret.click();
    pointer(slider, "pointerdown");
    slider.value = "2";
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    expect(onChoose).not.toHaveBeenCalled();
    expect(host.querySelector(".splitmenu").hidden).toBe(false);
    expect(host.querySelector('[data-group="compact"] .mt').textContent).toBe("200k");
    expect(slider.getAttribute("aria-valuetext")).toBe("200k");
    slider.value = "4";
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    slider.dispatchEvent(new Event("change", { bubbles: true }));
    expect(onChoose).not.toHaveBeenCalled();
    pointer(slider, "pointerup");
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:off");
    await motionBeat();
    expect(document.activeElement).toBe(slider);
    expect(caret.getAttribute("aria-expanded")).toBe("true");
    expect(host.querySelector(".splitmenu").hidden).toBe(false);
    keydown(slider, "Escape");
    expect(onChoose).toHaveBeenCalledTimes(1);
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
    for (const key of ["ArrowLeft", "ArrowRight"]) {
      expect(keydown(slider, key)).toBe(true);
      expect(document.activeElement).toBe(slider);
    }
    expect(onChoose).not.toHaveBeenCalled();
    keydown(slider, "Escape");
    await motionBeat();
    expect(host.querySelector(".splitmenu").hidden).toBe(true);
    expect(document.activeElement).toBe(caret);
  });

  it("previews keyboard changes until Enter and cancels them on Escape", async () => {
    const { caret, slider, host, onChoose } = mount();
    keydown(caret, "ArrowUp");
    for (const index of [1, 2, 3, 4]) {
      keydown(slider, "ArrowRight");
      preview(slider, index);
      expect(onChoose).not.toHaveBeenCalled();
      expect(document.activeElement).toBe(slider);
      expect(host.querySelector(".splitmenu").hidden).toBe(false);
    }
    keydown(slider, "Escape");
    await motionBeat();
    expect(onChoose).not.toHaveBeenCalled();
    expect(slider.value).toBe("0");
    keydown(caret, "ArrowUp");
    preview(slider, 4);
    keydown(slider, "Enter");
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:off");
    await motionBeat();
    expect(document.activeElement).toBe(slider);
    expect(host.querySelector(".splitmenu").hidden).toBe(false);
    expect(caret.getAttribute("aria-expanded")).toBe("true");
    keydown(slider, "Escape");
    expect(onChoose).toHaveBeenCalledTimes(1);
  });

  it("commits on thumb blur without stealing the menu navigation destination", () => {
    const { caret, slider, host, onChoose } = mount();
    keydown(caret, "ArrowUp");
    preview(slider, 3);
    keydown(slider, "ArrowUp");
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:300000");
    expect(document.activeElement.dataset.action).toBe("detail:all");
    expect(host.querySelector(".splitmenu").hidden).toBe(false);
    expect(slider.value).toBe("0");
  });

  it("commits on Tab blur once and keeps focus outside the menu", async () => {
    const { caret, slider, host, onChoose } = mount();
    const elsewhere = document.createElement("button");
    document.body.appendChild(elsewhere);
    keydown(caret, "ArrowUp");
    preview(slider, 2);
    elsewhere.focus();
    await motionBeat();
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:200000");
    expect(document.activeElement).toBe(elsewhere);
    expect(host.querySelector(".splitmenu").hidden).toBe(true);
  });

  it("commits a keyboard preview before an outside pointer press closes the menu", () => {
    const { caret, slider, onChoose } = mount();
    keydown(caret, "ArrowUp");
    preview(slider, 1);
    const elsewhere = document.createElement("button");
    document.body.appendChild(elsewhere);
    pointer(elsewhere, "pointerdown");
    elsewhere.focus();
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:150000");
    expect(document.activeElement).toBe(elsewhere);
  });

  it("walks and wraps the menu with Up/Down/Home/End from the thumb", () => {
    const { caret, slider, onChoose } = mount();
    keydown(caret, "ArrowUp");
    keydown(slider, "ArrowDown");
    expect(document.activeElement.dataset.action).toBe("detail:all");
    keydown(document.activeElement, "ArrowUp");
    expect(document.activeElement).toBe(slider);
    keydown(slider, "Home");
    expect(document.activeElement.dataset.action).toBe("detail:all");
    keydown(document.activeElement, "End");
    expect(document.activeElement).toBe(slider);
    expect(onChoose).not.toHaveBeenCalled();
  });

  it("does not reopen a closing slider menu when a cache reply repaints it", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const markup = (limit) => groupedMenuButtonMarkup("⋮", [compactionMenuGroup({ max_context_tokens: limit })]);
    const close = mountMenuIfChanged(host, markup(null), { onChoose: vi.fn() });
    keydown(host.querySelector(".caret"), "ArrowUp");
    close();
    mountMenuIfChanged(host, markup(150000), { onChoose: vi.fn() });
    expect(host.querySelector(".caret").getAttribute("aria-expanded")).toBe("false");
    expect(host.querySelector(".splitmenu").hidden).toBe(true);
  });

  it.each(["Enter", " "])("keeps slider focus after %s commits and a cache reply remounts it", (key) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const markup = (limit) => groupedMenuButtonMarkup("⋮", [compactionMenuGroup({ max_context_tokens: limit })]);
    const onChoose = vi.fn();
    mountMenuIfChanged(host, markup(null), { onChoose });
    keydown(host.querySelector(".caret"), "ArrowUp");
    const slider = host.querySelector('[role="slider"]');
    preview(slider, 1);
    keydown(slider, key);
    mountMenuIfChanged(host, markup(150000), { onChoose });
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:150000");
    expect(host.querySelector(".caret").getAttribute("aria-expanded")).toBe("true");
    expect(host.querySelector(".splitmenu").hidden).toBe(false);
    expect(document.activeElement).toBe(host.querySelector('[role="slider"]'));
    expect(document.activeElement.getAttribute("aria-valuetext")).toBe("150k");
  });

  it("can commit the standing stop with Enter", () => {
    const { caret, slider, onChoose } = mount();
    keydown(caret, "ArrowUp");
    keydown(slider, "Enter");
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:default");
  });

  it("restores the cached value when a preview is cancelled or a write is refused", async () => {
    const { caret, slider, host, onChoose } = mount();
    for (const event of ["Escape", "Enter"]) {
      caret.click();
      slider.focus();
      slider.value = "3";
      slider.dispatchEvent(new Event("input", { bubbles: true }));
      keydown(slider, event);
      await motionBeat();
      if (event === "Escape") caret.click();
      expect(slider.getAttribute("aria-valuetext")).toBe("Default (200k)");
      expect(slider.value).toBe("0");
      keydown(slider, "Escape");
      await motionBeat();
    }
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:300000");
    expect(host.querySelector('[data-group="compact"] .mt').textContent).toBe("Default (200k)");
  });
});
