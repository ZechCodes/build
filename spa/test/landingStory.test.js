// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { installCinematicStory } from "../../skriftapp/buildapp/landing/cinematic-story.js";
import { getDeviceFramePoses } from "../../skriftapp/buildapp/landing/device-stage.js";
import {
  STORY_SCENES,
  frameAtTravel,
  profileForViewport,
  storyTravel,
  travelAtFrame,
} from "../../skriftapp/buildapp/landing/story-manifest.js";

describe("landing story progression", () => {
  it("uses the approved travel totals and responsive static cutoff", () => {
    expect(storyTravel("desktop")).toBeCloseTo(4.5);
    expect(storyTravel("tablet")).toBeCloseTo(4.1);
    expect(storyTravel("compact")).toBeCloseTo(3.25);
    expect(profileForViewport(1440, 900)).toBe("desktop");
    expect(profileForViewport(900, 900)).toBe("tablet");
    expect(profileForViewport(768, 1024)).toBe("tablet");
    expect(profileForViewport(767, 1000)).toBe("static");
    expect(profileForViewport(390, 844)).toBe("static");
    expect(profileForViewport(844, 390)).toBe("static");
  });

  it("derives review evidence and approval from scroll position in both directions", () => {
    const reviewStart = travelAtFrame(4, 0, "desktop");
    const checkpoints = [0.28, 0.42, 0.58, 0.72, 0.82, 0.28].map(
      (local) => frameAtTravel(reviewStart + STORY_SCENES[4].travel.desktop * local, "desktop").checkpoint,
    );
    expect(checkpoints).toEqual(["summary", "diff", "evidence", "approval", "merged", "summary"]);
  });

  it("keeps device poses continuous at every scene boundary", () => {
    for (let index = 0; index < STORY_SCENES.length - 1; index += 1) {
      const before = getDeviceFramePoses(STORY_SCENES, { sceneIndex: index, local: 1, profile: "desktop" });
      const after = getDeviceFramePoses(STORY_SCENES, { sceneIndex: index + 1, local: 0, profile: "desktop" });
      expect(after).toEqual(before);
    }
    expect(getDeviceFramePoses(STORY_SCENES, { sceneIndex: 0, local: 0, profile: "desktop" }).laptop.w).toBeGreaterThan(0);
  });
});

describe("landing story browser behavior", () => {
  let motionListener;
  let motionMatches;

  beforeEach(() => {
    motionMatches = false;
    motionListener = undefined;
    Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 900, configurable: true });
    Object.defineProperty(window, "scrollY", { value: 0, writable: true, configurable: true });
    globalThis.matchMedia = vi.fn(() => ({
      get matches() { return motionMatches; },
      addEventListener: (_name, listener) => { motionListener = listener; },
      removeEventListener: vi.fn(),
    }));
    globalThis.requestAnimationFrame = (callback) => callback();
    window.scrollTo = vi.fn(({ top }) => { window.scrollY = top; });
    Element.prototype.scrollIntoView = vi.fn();
    document.body.innerHTML = `
      <main data-cinematic-page>
        <section data-story><div data-story-stage>
          ${STORY_SCENES.map((scene, index) => `<article id="${scene.id === "download" ? "download-story" : scene.id}" data-story-scene="${scene.id}"><h2>${scene.copy.title}</h2>${index === 5 ? '<a href="#download">Download</a>' : ""}</article>`).join("")}
          <span data-story-progress></span><span data-story-position></span>
        </div></section>
        <div id="download">Installer chooser</div>
      </main>`;
  });

  function install() {
    const story = document.querySelector("[data-story]");
    const stage = document.querySelector("[data-story-stage]");
    stage.getBoundingClientRect = () => ({ top: 0, height: 828, width: 1440, bottom: 828, left: 0, right: 1440 });
    story.getBoundingClientRect = () => ({ top: -window.scrollY, height: 6127, width: 1440, bottom: 6127 - window.scrollY, left: 0, right: 1440 });
    Object.defineProperty(story, "offsetHeight", { value: 6127, configurable: true });
    return installCinematicStory({ story, stage });
  }

  function installWithResponsiveGeometry() {
    const story = document.querySelector("[data-story]");
    const stage = document.querySelector("[data-story-stage]");
    stage.getBoundingClientRect = () => ({ top: 0, height: window.innerHeight - 72, width: window.innerWidth, bottom: window.innerHeight - 72, left: 0, right: window.innerWidth });
    Object.defineProperty(story, "offsetHeight", {
      configurable: true,
      get: () => Number.parseFloat(story.style.getPropertyValue("--story-height")) || 6127,
    });
    story.getBoundingClientRect = () => ({ top: -window.scrollY, height: story.offsetHeight, width: window.innerWidth, bottom: story.offsetHeight - window.scrollY, left: 0, right: window.innerWidth });
    return { controller: installCinematicStory({ story, stage }), story };
  }

  it("seeks a requested chapter and keeps inactive calls to action out of tab order", () => {
    const controller = install();
    controller.seek(5, 0.5);
    window.dispatchEvent(new Event("scroll"));
    expect(controller.getState().scene).toBe("download");
    expect(document.querySelector("#download-story a").getAttribute("tabindex")).toBeNull();
    controller.seek(0, 0.5);
    window.dispatchEvent(new Event("scroll"));
    expect(document.querySelector("#download-story a").getAttribute("tabindex")).toBe("-1");
    controller.destroy();
  });

  it("fades one scene fully out before the next one fades in", () => {
    const controller = install();
    const opacity = (index) => Number(document.querySelectorAll("[data-story-scene]")[index].style.getPropertyValue("--scene-opacity"));
    const entered = (index) => Number(document.querySelectorAll("[data-story-scene]")[index].style.getPropertyValue("--scene-enter"));
    controller.seek(0, 0.85);
    window.dispatchEvent(new Event("scroll"));
    expect(opacity(0)).toBeCloseTo(0.5);
    expect(opacity(1)).toBe(0);
    controller.seek(0, 0.95);
    window.dispatchEvent(new Event("scroll"));
    expect(opacity(0)).toBe(0);
    expect(opacity(1)).toBeCloseTo(0.5);
    expect(entered(1)).toBeCloseTo(0.5);
    controller.seek(1, 0.5);
    window.dispatchEvent(new Event("scroll"));
    expect(opacity(1)).toBe(1);
    expect(entered(1)).toBe(1);
    controller.destroy();
  });

  it("switches to readable document flow when reduced motion changes", () => {
    const controller = install();
    expect(controller.getState().enhanced).toBe(true);
    motionMatches = true;
    motionListener({ matches: true });
    expect(controller.getState().enhanced).toBe(false);
    expect(document.querySelector("[data-story]").dataset.storyMode).toBe("static");
    expect(document.querySelector("#download-story a").getAttribute("tabindex")).toBeNull();
    controller.destroy();
  });

  it("returns to the top of the page when document flow starts at the first scene", () => {
    const controller = install();
    window.scrollY = 300;
    window.dispatchEvent(new Event("scroll"));
    expect(controller.getState().scene).toBe("start");
    motionMatches = true;
    motionListener({ matches: true });
    expect(controller.getState().enhanced).toBe(false);
    expect(window.scrollY).toBe(0);
    expect(Element.prototype.scrollIntoView).not.toHaveBeenCalled();
    controller.destroy();
  });

  it("keeps content after the pinned travel anchored during a viewport resize", () => {
    const { controller, story } = installWithResponsiveGeometry();
    window.scrollY = story.offsetHeight;
    Object.defineProperty(window, "innerHeight", { value: 860, configurable: true });
    window.dispatchEvent(new Event("resize"));
    expect(window.scrollY).toBeCloseTo(story.offsetHeight);
    expect(controller.getState().scene).toBe("download");
    controller.destroy();
  });
});
