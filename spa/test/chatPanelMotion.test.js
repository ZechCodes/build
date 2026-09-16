// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CHAT_LAYOUT_TRANSITION_MS,
  CHAT_PANEL_TRANSITION_MS,
  createChatPanelMotion,
} from "../src/core/chatPanelMotion.js";

const rect = (left, top, width, height) => ({ left, top, width, height });

let host;
let panel;
let scroller;
let target;
let animations;

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("matchMedia", vi.fn(() => ({ matches: false })));
  host = document.createElement("div");
  panel = document.createElement("section");
  scroller = document.createElement("div");
  panel.append(scroller);
  host.append(panel);
  document.body.append(host);
  target = rect(600, 20, 420, 700);
  panel.getBoundingClientRect = vi.fn(() => panel.style.position === "fixed"
    ? rect(
      Number.parseFloat(panel.style.left),
      Number.parseFloat(panel.style.top),
      Number.parseFloat(panel.style.width),
      Number.parseFloat(panel.style.height),
    )
    : target);
  animations = [];
  panel.animate = vi.fn((frames, options) => {
    const animation = { frames, options, cancel: vi.fn(), onfinish: null, oncancel: null };
    animation.cancel.mockImplementation(() => animation.oncancel?.());
    animations.push(animation);
    return animation;
  });
});

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("the conversation panel geometry transition", () => {
  it("holds the live panel while layout settles, then moves that node to its measured card", () => {
    const phases = [];
    const motion = createChatPanelMotion(host, { onPhase: (phase) => phases.push(phase) });
    motion.run({
      panel,
      scroller,
      direction: "popover",
      apply: () => {
        target = rect(500, 30, 380, 560);
      },
    });

    expect(panel.style.position).toBe("fixed");
    expect(panel.style.left).toBe("600px");
    expect(host.dataset.panelTransitionPhase).toBe("layout");
    expect(animations).toHaveLength(0);

    vi.advanceTimersByTime(CHAT_LAYOUT_TRANSITION_MS);
    expect(animations).toHaveLength(1);
    expect(animations[0].frames).toEqual([
      { left: "600px", top: "20px", width: "420px", height: "700px" },
      { left: "500px", top: "30px", width: "380px", height: "560px" },
    ]);
    expect(animations[0].options).toEqual({
      duration: CHAT_PANEL_TRANSITION_MS,
      easing: "cubic-bezier(.2,.8,.2,1)",
    });
    expect(host.dataset.panelTransitionPhase).toBe("panel");

    animations[0].onfinish();
    expect(panel.style.position).toBe("");
    expect(host.dataset.panelTransition).toBeUndefined();
    expect(phases).toEqual(["idle", "layout", "panel", "idle"]);
  });

  it("cancels the first wait when the pin is rapidly reversed", () => {
    const motion = createChatPanelMotion(host);
    motion.run({ panel, direction: "popover", apply: () => { target = rect(500, 30, 380, 560); } });
    vi.advanceTimersByTime(60);
    motion.run({ panel, direction: "pinned", apply: () => { target = rect(600, 20, 420, 700); } });

    vi.advanceTimersByTime(CHAT_LAYOUT_TRANSITION_MS - 60);
    expect(animations).toHaveLength(0);
    vi.advanceTimersByTime(60);
    expect(animations).toHaveLength(1);
    expect(host.dataset.panelTransition).toBe("pinned");
  });

  it("releases pin geometry when the panel is collapsed mid-transition", () => {
    const motion = createChatPanelMotion(host);
    motion.run({ panel, direction: "popover", apply: () => { target = rect(500, 30, 380, 560); } });

    motion.setVisible({ panel, visible: false, apply: () => panel.setAttribute("aria-hidden", "true") });

    expect(panel.style.position).toBe("");
    vi.advanceTimersByTime(CHAT_LAYOUT_TRANSITION_MS);
    expect(animations).toHaveLength(0);
  });

  it("releases fixed geometry when dismissal, resize, navigation, or disposal cancels it", () => {
    const motion = createChatPanelMotion(host);
    motion.run({ panel, direction: "popover", apply: () => { target = rect(500, 30, 380, 560); } });
    motion.cancel();

    expect(panel.style.position).toBe("");
    expect(host.dataset.panelTransition).toBeUndefined();
    vi.runAllTimers();
    expect(animations).toHaveLength(0);
  });

  it("applies reduced-motion pin changes immediately", () => {
    matchMedia.mockReturnValue({ matches: true });
    const apply = vi.fn();
    const motion = createChatPanelMotion(host);
    motion.run({ panel, direction: "popover", apply });

    expect(apply).toHaveBeenCalledOnce();
    expect(panel.style.position).toBe("");
    expect(host.dataset.panelTransition).toBeUndefined();
    expect(animations).toHaveLength(0);
  });

  it("keeps a bottom-pinned conversation at the bottom without motion", () => {
    matchMedia.mockReturnValue({ matches: true });
    let bottom = 200;
    Object.defineProperties(scroller, {
      clientHeight: { configurable: true, value: 100 },
      scrollHeight: { configurable: true, get: () => bottom + 100 },
    });
    scroller.scrollTop = bottom;
    const motion = createChatPanelMotion(host);
    motion.run({
      panel,
      scroller,
      direction: "popover",
      apply: () => {
        bottom = 380;
      },
    });

    // Browsers clamp this request to their maximum (380 here); jsdom records
    // the requested scrollHeight verbatim because it has no layout engine.
    expect(scroller.scrollTop).toBe(480);
  });

  it("conceals a collapsed panel only after its exit transition", () => {
    const apply = vi.fn(() => panel.setAttribute("aria-hidden", "true"));
    const motion = createChatPanelMotion(host);

    motion.setVisible({ panel, visible: false, apply });

    expect(apply).toHaveBeenCalledOnce();
    expect(panel.classList.contains("rail-panel-concealed")).toBe(false);
    vi.advanceTimersByTime(CHAT_PANEL_TRANSITION_MS);
    expect(panel.classList.contains("rail-panel-concealed")).toBe(true);
  });

  it("animates the first reveal from a measured concealed frame", () => {
    const motion = createChatPanelMotion(host);
    motion.setVisible({ panel, visible: true, opening: true, apply: () => panel.setAttribute("aria-hidden", "false") });

    expect(animations).toHaveLength(1);
    expect(animations[0].frames[0].opacity).toBe("0");
    expect(animations[0].frames[1]).toEqual({ opacity: "1", transform: "none" });
    expect(animations[0].options.duration).toBe(CHAT_PANEL_TRANSITION_MS);
  });

  it("cancels a stale conceal when collapse is rapidly reversed", () => {
    const motion = createChatPanelMotion(host);
    motion.setVisible({ panel, visible: false, apply: () => panel.setAttribute("aria-hidden", "true") });
    vi.advanceTimersByTime(CHAT_PANEL_TRANSITION_MS / 2);
    motion.setVisible({ panel, visible: true, apply: () => panel.setAttribute("aria-hidden", "false") });
    vi.runAllTimers();

    expect(panel.getAttribute("aria-hidden")).toBe("false");
    expect(panel.classList.contains("rail-panel-concealed")).toBe(false);
  });

  it("conceals immediately when reduced motion is requested", () => {
    matchMedia.mockReturnValue({ matches: true });
    const motion = createChatPanelMotion(host);
    motion.setVisible({ panel, visible: false, apply: () => panel.setAttribute("aria-hidden", "true") });

    expect(panel.classList.contains("rail-panel-concealed")).toBe(true);
  });
});
