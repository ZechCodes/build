// @vitest-environment jsdom
import { afterEach, expect, it } from "vitest";
import { installWindowControlsOverlay } from "../src/core/windowControlsOverlay.js";

afterEach(() => {
  delete navigator.windowControlsOverlay;
  document.documentElement.removeAttribute("data-window-controls-overlay");
  document.documentElement.style.removeProperty("--window-controls-x");
});

it("does nothing in browsers without Window Controls Overlay", () => {
  const before = document.documentElement.outerHTML;
  const stop = installWindowControlsOverlay();
  stop();
  expect(document.documentElement.outerHTML).toBe(before);
});

it("clears stale geometry on empty fullscreen rects and stops listening on disposal", () => {
  const overlay = new EventTarget();
  overlay.visible = true;
  let rect = { x: 84, width: 1196, height: 44 };
  overlay.getTitlebarAreaRect = () => rect;
  Object.defineProperty(navigator, "windowControlsOverlay", { configurable: true, value: overlay });
  const root = document.documentElement;
  const stop = installWindowControlsOverlay();
  expect(root.hasAttribute("data-window-controls-overlay")).toBe(true);
  expect(root.style.getPropertyValue("--window-controls-x")).toBe("84px");
  rect = { x: 0, width: 0, height: 0 };
  overlay.dispatchEvent(new Event("geometrychange"));
  expect(root.hasAttribute("data-window-controls-overlay")).toBe(false);
  expect(root.style.getPropertyValue("--window-controls-x")).toBe("");
  rect = { x: 104, width: 1176, height: 44 };
  overlay.dispatchEvent(new Event("geometrychange"));
  expect(root.style.getPropertyValue("--window-controls-x")).toBe("104px");
  stop();
  overlay.dispatchEvent(new Event("geometrychange"));
  expect(root.hasAttribute("data-window-controls-overlay")).toBe(false);
  expect(root.style.getPropertyValue("--window-controls-x")).toBe("");
});
