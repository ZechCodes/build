/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { installMarkdownTableFocus } from "../src/core/markdownTableFocus.js";

let resize;
let dispose;
const observed = new Set();
const disconnected = vi.fn();
beforeEach(() => {
  observed.clear();
  disconnected.mockClear();
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback) { resize = callback; }
    observe(element) { observed.add(element); }
    unobserve(element) { observed.delete(element); }
    disconnect() { disconnected(); observed.clear(); }
  });
});
afterEach(() => { dispose?.(); document.body.innerHTML = ""; vi.unstubAllGlobals(); });
const table = (width = 200) => {
  const wrapper = document.createElement("div");
  wrapper.className = "mdtable";
  wrapper.innerHTML = "<table><tbody><tr><td>Words</td></tr></tbody></table>";
  Object.defineProperties(wrapper, { clientWidth: { value: width, configurable: true }, scrollWidth: { value: width, configurable: true } });
  return wrapper;
};
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

it("measures overflow before adding a stop and removes it when the wrapper fits", () => {
  const wrapper = table();
  document.body.append(wrapper);
  dispose = installMarkdownTableFocus(document);
  expect(wrapper.hasAttribute("tabindex")).toBe(false);
  expect(observed.has(wrapper)).toBe(true);
  expect(observed.has(wrapper.querySelector("table"))).toBe(true);
  Object.defineProperty(wrapper, "scrollWidth", { value: 400, configurable: true });
  resize([{ target: wrapper.querySelector("table") }]);
  expect(wrapper.getAttribute("tabindex")).toBe("0");
  Object.defineProperty(wrapper, "clientWidth", { value: 400, configurable: true });
  resize([{ target: wrapper }]);
  expect(wrapper.hasAttribute("tabindex")).toBe(false);
});

it("tracks newly painted tables, releases removed nodes, and stops all observers on disposal", async () => {
  dispose = installMarkdownTableFocus(document);
  const wrapper = table();
  Object.defineProperty(wrapper, "scrollWidth", { value: 400, configurable: true });
  document.body.append(wrapper);
  await settle();
  expect(wrapper.getAttribute("tabindex")).toBe("0");
  const original = wrapper.querySelector("table");
  wrapper.innerHTML = "<table><tbody><tr><td>Replacement</td></tr></tbody></table>";
  await settle();
  expect(observed.has(original)).toBe(false);
  expect(observed.has(wrapper.querySelector("table"))).toBe(true);
  wrapper.remove();
  await settle();
  expect(observed.size).toBe(0);
  expect(wrapper.hasAttribute("tabindex")).toBe(false);
  dispose();
  expect(disconnected).toHaveBeenCalledTimes(1);
  document.body.append(wrapper);
  resize([{ target: wrapper }]);
  await settle();
  expect(observed.size).toBe(0);
  expect(wrapper.hasAttribute("tabindex")).toBe(false);
  dispose = null;
});
