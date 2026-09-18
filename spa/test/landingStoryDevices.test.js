// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { frameBudgetAction } from "../../skriftapp/buildapp/landing/device-stage.js";
import { installStoryDevices } from "../../skriftapp/buildapp/landing/story-devices.js";

describe("device stage frame budget", () => {
  it("drops render quality once, then falls back to posters, and never fails the story", () => {
    let state = { renderCount: 10, slowFrameCount: 0, qualityScale: 1 };
    const actions = [];
    for (let frame = 0; frame < 9; frame += 1) {
      const next = frameBudgetAction(state, 40);
      actions.push(next.action);
      state = { ...state, slowFrameCount: next.slowFrameCount, qualityScale: next.qualityScale };
    }
    expect(actions).toEqual(["none", "none", "none", "reduce", "none", "none", "none", "posters", "posters"]);
    expect(actions).not.toContain("fail");
  });

  it("ignores the warm-up frames and resets after a fast frame", () => {
    expect(frameBudgetAction({ renderCount: 2, slowFrameCount: 0, qualityScale: 1 }, 90).action).toBe("none");
    const slow = frameBudgetAction({ renderCount: 10, slowFrameCount: 3, qualityScale: 1 }, 10);
    expect(slow).toMatchObject({ action: "none", slowFrameCount: 0 });
  });
});

describe("story devices", () => {
  let story;
  let storyStage;
  let deviceStage;

  beforeEach(() => {
    vi.useRealTimers();
    globalThis.matchMedia = vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }));
    delete window.requestIdleCallback;
    document.body.innerHTML = `
      <div data-story-stage>
        <div data-device-stage>
          <img data-device="laptop" data-src="/landing/assets/devices/laptop.webp" alt="">
          <img data-device="tablet" data-src="/landing/assets/devices/tablet.webp" alt="">
          <img data-device="phone" data-src="/landing/assets/devices/phone.webp" alt="">
        </div>
        <div data-review-surface></div>
      </div>`;
    storyStage = document.querySelector("[data-story-stage]");
    deviceStage = document.querySelector("[data-device-stage]");
    const frame = { scene: "start", sceneIndex: 0, local: 0.5, progress: 0, profile: "desktop", checkpoint: "request", enhanced: true };
    story = {
      getState: () => ({ ...frame }),
      subscribe: vi.fn((subscriber) => { subscriber(frame); return () => {}; }),
      useStatic: vi.fn(),
    };
  });

  it("keeps the pinned story and its posters when the WebGL renderer cannot start", async () => {
    const devices = installStoryDevices({ story, storyStage, deviceStage });
    await vi.waitFor(() => expect(devices.getState().destroyed).toBe(true), { timeout: 4000 });
    expect(story.useStatic).not.toHaveBeenCalled();
    expect(deviceStage.classList.contains("device-stage--enhanced")).toBe(false);
    const laptop = deviceStage.querySelector('[data-device="laptop"]');
    expect(laptop.getAttribute("src")).toBe("/landing/assets/devices/laptop.webp");
    expect(laptop.style.getPropertyValue("opacity")).toBe("");
    expect(laptop.style.getPropertyValue("--device-x")).not.toBe("");
  });
});
