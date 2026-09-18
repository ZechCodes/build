// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createChatTitleMotion, titleGraphemes } from "../src/core/chatTitleMotion.js";

const title = (text, over = {}) => ({ text, title: "Claude Code 1", starting: false, ...over });

let element;

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("matchMedia", vi.fn(() => ({ matches: false })));
  element = document.createElement("span");
  element.className = "rail-who";
  element.title = "Claude Code 1";
  element.textContent = "Old";
  document.body.append(element);
});

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("chat title motion", () => {
  it("erases the old title completely before typing the new one", () => {
    const motion = createChatTitleMotion(element);

    motion.show(title("New topic"));
    expect(element.textContent).toBe("Old");

    vi.advanceTimersToNextTimer();
    expect(element.textContent).toBe("Ol");
    vi.advanceTimersToNextTimer();
    expect(element.textContent).toBe("O");
    vi.advanceTimersToNextTimer();
    expect(element.textContent).toBe("");

    vi.advanceTimersToNextTimer();
    expect(element.textContent).toBe("N");
    vi.runAllTimers();
    expect(element.textContent).toBe("New topic");
  });

  it("coalesces updates received while erasing into the latest title", () => {
    const motion = createChatTitleMotion(element);

    motion.show(title("First topic"));
    vi.advanceTimersToNextTimer();
    motion.show(title("Latest topic"));
    vi.runAllTimers();

    expect(element.textContent).toBe("Latest topic");
  });

  it("turns around from a partly typed stale title and finishes on the latest update", () => {
    const motion = createChatTitleMotion(element);

    motion.show(title("First"));
    while (element.dataset.titleMotion !== "typing") vi.advanceTimersToNextTimer();
    vi.advanceTimersToNextTimer();
    expect(element.textContent).toBe("F");

    motion.show(title("Latest"));
    vi.advanceTimersToNextTimer();
    expect(element.textContent).toBe("");
    vi.advanceTimersToNextTimer();
    expect(element.textContent).toBe("L");
    vi.runAllTimers();

    expect(element.textContent).toBe("Latest");
  });

  it("never cuts through a Unicode grapheme while erasing", () => {
    element.textContent = "A👨‍👩‍👧‍👦B";
    const motion = createChatTitleMotion(element);

    motion.show(title("Done"));
    vi.advanceTimersToNextTimer();
    expect(element.textContent).toBe("A👨‍👩‍👧‍👦");
    vi.advanceTimersToNextTimer();
    expect(element.textContent).toBe("A");
    vi.advanceTimersToNextTimer();
    expect(element.textContent).toBe("");
  });

  it("falls back to an immediate Unicode-safe update without Intl.Segmenter", () => {
    const intl = globalThis.Intl;
    vi.stubGlobal("Intl", { ...intl, Segmenter: undefined });
    const motion = createChatTitleMotion(element);

    expect(titleGraphemes("e\u0301 👨‍👩‍👧‍👦 🇺🇸")).toBe(null);
    motion.show(title("e\u0301 👨‍👩‍👧‍👦 🇺🇸"));
    expect(element.textContent).toBe("e\u0301 👨‍👩‍👧‍👦 🇺🇸");
    expect(vi.getTimerCount()).toBe(0);

    vi.stubGlobal("Intl", intl);
  });

  it("starts erasing at the visible end of an ellipsized long title", () => {
    element.textContent = "A very long title whose suffix is outside the header";
    Object.defineProperties(element, {
      clientWidth: { configurable: true, value: 80 },
      scrollWidth: { configurable: true, value: 400 },
    });
    const motion = createChatTitleMotion(element);

    motion.show(title("Replacement"));
    vi.advanceTimersToNextTimer();

    expect(element.textContent.length).toBeLessThan(15);
    expect("A very long title whose suffix is outside the header".startsWith(element.textContent)).toBe(true);
  });

  it("starts visibly erasing when a wide prefix is followed by a narrow hidden suffix", () => {
    const widthOf = (text) => [...text].reduce((width, glyph) => width + (glyph === "W" ? 4 : 1), 0);
    element.textContent = "WWiiiiiiii";
    Object.defineProperties(element, {
      clientWidth: { configurable: true, value: 10 },
      scrollWidth: { configurable: true, get: () => Math.max(10, widthOf(element.textContent)) },
    });
    const motion = createChatTitleMotion(element);

    motion.show(title("Replacement"));
    vi.advanceTimersToNextTimer();

    expect(element.textContent).toBe("WWi");
  });

  it("types only the visible prefix before settling a long ellipsized title", () => {
    Object.defineProperties(element, {
      clientWidth: { configurable: true, value: 80 },
      scrollWidth: { configurable: true, get: () => element.textContent.length * 10 },
    });
    const replacement = "A replacement title with a long hidden suffix";
    const motion = createChatTitleMotion(element);

    motion.show(title(replacement));
    while (element.dataset.titleMotion !== "typing") vi.advanceTimersToNextTimer();
    vi.advanceTimersToNextTimer();

    expect(element.textContent).toBe("A");
    vi.runAllTimers();
    expect(element.textContent).toBe(replacement);
  });

  it("types a narrow prefix close to the visible edge before restoring a wide hidden suffix", () => {
    const widthOf = (text) => [...text].reduce((width, glyph) => width + (glyph === "W" ? 4 : 1), 0);
    Object.defineProperties(element, {
      clientWidth: { configurable: true, value: 20 },
      scrollWidth: { configurable: true, get: () => Math.max(20, widthOf(element.textContent)) },
    });
    const replacement = "iiiiiiiiiiiiiiiiiiiiWW";
    const motion = createChatTitleMotion(element);

    motion.show(title(replacement));
    while (element.dataset.titleMotion !== "typing") vi.advanceTimersToNextTimer();
    let longestAnimatedPrefix = "";
    for (let step = 0; step < 20 && element.dataset.titleMotion === "typing"; step += 1) {
      vi.advanceTimersToNextTimer();
      if (element.dataset.titleMotion === "typing" && element.textContent.length > longestAnimatedPrefix.length) {
        longestAnimatedPrefix = element.textContent;
      }
    }

    // A character-count ratio would stop at sixteen narrow glyphs here. Width
    // fitting carries the visible animation almost to the element's edge.
    expect(longestAnimatedPrefix).toBe("iiiiiiiiiiiiiiiiii");
    expect(element.textContent).toBe(replacement);
  });

  it("cancels pending work when disposed", () => {
    const motion = createChatTitleMotion(element);

    motion.show(title("New topic"));
    vi.advanceTimersToNextTimer();
    const stoppedAt = element.textContent;
    motion.dispose();
    vi.runAllTimers();

    expect(element.textContent).toBe(stoppedAt);
    expect(element.dataset.titleMotion).toBeUndefined();
  });

  it("applies updates immediately for reduced motion and hidden titles", () => {
    matchMedia.mockReturnValue({ matches: true });
    const reduced = createChatTitleMotion(element);
    reduced.show(title("Reduced", { starting: true }));

    expect(element.textContent).toBe("Reduced");
    expect(element.classList.contains("rail-who-starting")).toBe(true);
    expect(vi.getTimerCount()).toBe(0);

    matchMedia.mockReturnValue({ matches: false });
    element.parentElement.hidden = true;
    const hidden = createChatTitleMotion(element);
    hidden.show(title("Hidden"));

    expect(element.textContent).toBe("Hidden");
    expect(vi.getTimerCount()).toBe(0);
  });

  it("settles when reduced motion or panel concealment arrives mid-transition", () => {
    let reduced = false;
    matchMedia.mockImplementation(() => ({ matches: reduced }));
    const motion = createChatTitleMotion(element);
    motion.show(title("Reduced midway"));
    vi.advanceTimersToNextTimer();

    reduced = true;
    vi.advanceTimersToNextTimer();
    expect(element.textContent).toBe("Reduced midway");
    expect(vi.getTimerCount()).toBe(0);

    reduced = false;
    const panel = document.createElement("section");
    panel.append(element);
    document.body.append(panel);
    motion.show(title("Concealed midway"));
    vi.advanceTimersToNextTimer();
    panel.setAttribute("aria-hidden", "true");
    vi.advanceTimersToNextTimer();

    expect(element.textContent).toBe("Concealed midway");
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does nothing when the title text is unchanged", () => {
    const motion = createChatTitleMotion(element);
    motion.show(title("Old", { title: "Codex 1" }));

    expect(element.textContent).toBe("Old");
    expect(element.title).toBe("Codex 1");
    expect(vi.getTimerCount()).toBe(0);
  });
});
