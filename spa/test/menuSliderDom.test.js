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

function mount(agent = { max_context_tokens: null, compact_at_tokens: 200000 }, onChoose = vi.fn()) {
  const host = document.createElement("div");
  host.innerHTML = groupedMenuButtonMarkup("⋮", [
    { id: "detail", label: "Detail", options: [{ id: "detail:all", label: "All", description: "Everything", selected: true }] },
    compactionMenuGroup(agent),
  ], { title: "Conversation menu", icon: true });
  document.body.appendChild(host);
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
    expect(slider.value).toBe("3");
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

  it.each(["Enter", " "])("keeps the same focused slider and menu after %s commits and a cache reply patches it", async (key) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const markup = (limit) => groupedMenuButtonMarkup("⋮", [compactionMenuGroup({ max_context_tokens: limit })]);
    const onChoose = vi.fn();
    mountMenuIfChanged(host, markup(null), { onChoose });
    keydown(host.querySelector(".caret"), "ArrowUp");
    const slider = host.querySelector('[role="slider"]');
    const menu = host.querySelector(".splitmenu");
    await motionBeat();
    const mutations = [];
    const observer = new MutationObserver((records) => mutations.push(...records));
    observer.observe(host, { subtree: true, childList: true, attributes: true, attributeOldValue: true });
    const blur = vi.fn();
    slider.addEventListener("blur", blur);
    preview(slider, 1);
    keydown(slider, key);
    mountMenuIfChanged(host, markup(150000), { onChoose });
    await motionBeat();
    observer.disconnect();
    expect(host.querySelector(".splitmenu")).toBe(menu);
    expect(host.querySelector('[role="slider"]')).toBe(slider);
    expect(blur).not.toHaveBeenCalled();
    expect(mutations.filter((record) => record.attributeName === "hidden")).toEqual([]);
    expect(mutations.filter((record) => record.attributeName === "aria-expanded")).toEqual([]);
    expect(mutations.some((record) => [...record.removedNodes].some((node) => node === menu || node.contains(menu)))).toBe(false);
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:150000");
    expect(host.querySelector(".caret").getAttribute("aria-expanded")).toBe("true");
    expect(host.querySelector(".splitmenu").hidden).toBe(false);
    expect(document.activeElement).toBe(host.querySelector('[role="slider"]'));
    expect(document.activeElement.getAttribute("aria-valuetext")).toBe("150k");
  });

  it("preserves a drag and its preview while another row is repainted", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const markup = (label) => groupedMenuButtonMarkup("⋮", [
      { id: "detail", label: "Detail", options: [{ id: "detail:all", label }] },
      compactionMenuGroup({ max_context_tokens: null, compact_at_tokens: 200000 }),
    ]);
    const onChoose = vi.fn();
    mountMenuIfChanged(host, markup("All"), { onChoose });
    keydown(host.querySelector(".caret"), "ArrowUp");
    const slider = host.querySelector('[role="slider"]');
    pointer(slider, "pointerdown");
    preview(slider, 3);
    mountMenuIfChanged(host, markup("All activity"), { onChoose });
    expect(host.querySelector('[role="slider"]')).toBe(slider);
    expect(slider.getAttribute("aria-valuetext")).toBe("300k");
    pointer(slider, "pointerup");
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:300000");
  });

  it("lets a changed cache win over a pending choice and its later refusal", async () => {
    let refuse;
    const onChoose = vi.fn(() => new Promise((resolve) => { refuse = () => resolve(false); }));
    const host = document.createElement("div");
    document.body.appendChild(host);
    const markup = (limit) => groupedMenuButtonMarkup("⋮", [compactionMenuGroup({ max_context_tokens: limit })]);
    mountMenuIfChanged(host, markup(null), { onChoose });
    keydown(host.querySelector(".caret"), "ArrowUp");
    const slider = host.querySelector('[role="slider"]');
    preview(slider, 1);
    keydown(slider, "Enter");
    mountMenuIfChanged(host, markup(300000), { onChoose });
    expect(host.querySelector('[role="slider"]')).toBe(slider);
    expect(slider.getAttribute("aria-valuetext")).toBe("300k");
    refuse();
    await motionBeat();
    expect(slider.getAttribute("aria-valuetext")).toBe("300k");
    keydown(slider, "Enter");
    expect(onChoose).toHaveBeenCalledTimes(1);
  });

  it("keeps a custom stop's slider node when a save removes its extra tick", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const markup = (limit) => groupedMenuButtonMarkup("⋮", [compactionMenuGroup({ max_context_tokens: limit })]);
    const onChoose = vi.fn();
    mountMenuIfChanged(host, markup(250000), { onChoose });
    keydown(host.querySelector(".caret"), "ArrowUp");
    const slider = host.querySelector('[role="slider"]');
    preview(slider, 3);
    keydown(slider, "Enter");
    mountMenuIfChanged(host, markup(300000), { onChoose });
    expect(host.querySelector('[role="slider"]')).toBe(slider);
    expect(slider.max).toBe("4");
    expect(host.querySelectorAll(".menu-slider-stops span")).toHaveLength(5);
    await motionBeat();
    preview(slider, 4);
    pointer(slider, "pointerdown");
    pointer(slider, "pointerup");
    expect(onChoose).toHaveBeenLastCalledWith("compact:off");
  });

  it("keeps a detached save's refusal from repainting a replacement slider", async () => {
    let refuse;
    const onChoose = vi.fn(() => new Promise((resolve) => { refuse = () => resolve(false); }));
    const host = document.createElement("div");
    document.body.appendChild(host);
    const markup = (label) => groupedMenuButtonMarkup("⋮", [
      { id: "detail", label, options: [{ id: "detail:all", label: "All" }] },
      compactionMenuGroup({ max_context_tokens: null, compact_at_tokens: 200000 }),
    ]);
    const close = mountMenuIfChanged(host, markup("Detail"), { onChoose });
    keydown(host.querySelector(".caret"), "ArrowUp");
    const oldSlider = host.querySelector('[role="slider"]');
    preview(oldSlider, 1);
    keydown(oldSlider, "Enter");
    close();
    mountMenuIfChanged(host, markup("Activity detail"), { onChoose });
    keydown(host.querySelector(".caret"), "ArrowUp");
    const newSlider = host.querySelector('[role="slider"]');
    preview(newSlider, 3);
    refuse();
    await motionBeat();
    expect(newSlider).not.toBe(oldSlider);
    expect(document.activeElement).toBe(newSlider);
    expect(newSlider.getAttribute("aria-valuetext")).toBe("300k");
    expect(host.querySelector('.menu-slider .mt').textContent).toBe("300k");
  });

  it("does not resend the standing stop with Enter", () => {
    const { caret, slider, onChoose } = mount();
    keydown(caret, "ArrowUp");
    keydown(slider, "Enter");
    expect(onChoose).not.toHaveBeenCalled();
  });

  it("keeps one committed stop through further previews, Escape, blur and an outside press while saving", async () => {
    let refuse;
    const onChoose = vi.fn(() => new Promise((_resolve, reject) => { refuse = reject; }));
    const { caret, slider, host } = mount(undefined, onChoose);
    keydown(caret, "ArrowUp");
    preview(slider, 1);
    keydown(slider, "Enter");
    expect(slider.getAttribute("aria-valuetext")).toBe("150k");
    preview(slider, 3);
    keydown(slider, "Enter");
    expect(slider.getAttribute("aria-valuetext")).toBe("150k");
    preview(slider, 4);
    keydown(slider, "Escape");
    slider.blur();
    pointer(document.body, "pointerdown");
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:150000");
    refuse(new Error("refused"));
    await motionBeat();
    expect(slider.getAttribute("aria-valuetext")).toBe("Default (200k)");
    expect(host.querySelector(".splitmenu").hidden).toBe(true);
  });

  it("keeps a pending save across an unrelated menu repaint and rolls back the current thumb on refusal", async () => {
    let refuse;
    const onChoose = vi.fn(() => new Promise((resolve) => { refuse = () => resolve(false); }));
    const host = document.createElement("div");
    document.body.appendChild(host);
    const markup = (label) => groupedMenuButtonMarkup("⋮", [
      { id: "detail", label: "Detail", options: [{ id: "detail:all", label }] },
      compactionMenuGroup({ max_context_tokens: null, compact_at_tokens: 200000 }),
    ]);
    mountMenuIfChanged(host, markup("All"), { onChoose });
    keydown(host.querySelector(".caret"), "ArrowUp");
    preview(host.querySelector('[role="slider"]'), 1);
    keydown(document.activeElement, "Enter");
    mountMenuIfChanged(host, markup("All activity"), { onChoose });
    expect(document.activeElement.getAttribute("aria-valuetext")).toBe("150k");
    expect(host.querySelector('.menu-slider .mt').textContent).toBe("150k");
    keydown(document.activeElement, "Enter");
    expect(onChoose).toHaveBeenCalledExactlyOnceWith("compact:150000");
    refuse();
    await motionBeat();
    expect(document.activeElement.getAttribute("aria-valuetext")).toBe("Default (200k)");
    expect(host.querySelector('.menu-slider .mt').textContent).toBe("Default (200k)");
    expect(host.querySelector(".caret").getAttribute("aria-expanded")).toBe("true");
  });

  it("restores the cached value when a preview is cancelled or a write is refused", async () => {
    const { caret, slider, host, onChoose } = mount(undefined, vi.fn(async () => false));
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
