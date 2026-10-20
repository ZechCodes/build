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
afterEach(() => { dispose?.(); document.body.innerHTML = ""; vi.unstubAllGlobals(); vi.restoreAllMocks(); });
const table = (width = 200) => {
  const wrapper = document.createElement("div");
  wrapper.className = "mdtable";
  wrapper.innerHTML = "<table><tbody><tr><td>Words</td></tr></tbody></table>";
  Object.defineProperties(wrapper, { clientWidth: { value: width, configurable: true }, scrollWidth: { value: width, configurable: true } });
  return wrapper;
};
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const readsFor = (wrapper) => {
  const client = vi.fn(() => 200);
  const scroll = vi.fn(() => 400);
  Object.defineProperties(wrapper, {
    clientWidth: { get: client, configurable: true },
    scrollWidth: { get: scroll, configurable: true },
  });
  return {
    client, scroll,
    clear() { client.mockClear(); scroll.mockClear(); },
    expectNone() { expect(client).not.toHaveBeenCalled(); expect(scroll).not.toHaveBeenCalled(); },
  };
};

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

it("ignores streaming text, timer ticks and unrelated DOM without scanning or measuring existing tables", async () => {
  const wrapper = table();
  const reads = readsFor(wrapper);
  const stream = document.createElement("p");
  stream.append("stream");
  const timer = document.createElement("span");
  timer.append("00:01");
  document.body.append(wrapper, stream, timer);
  dispose = installMarkdownTableFocus(document);
  reads.clear();
  const rootScan = vi.spyOn(document, "querySelectorAll");
  stream.firstChild.data += " chunk";
  timer.textContent = "00:02";
  stream.append(document.createTextNode(" next chunk"));
  const unrelated = document.createElement("section");
  unrelated.innerHTML = "<p>Unrelated repaint</p><table><tr><td>Native settings table</td></tr></table>";
  document.body.append(unrelated);
  await settle();
  unrelated.remove();
  await settle();
  reads.expectNone();
  expect(rootScan).not.toHaveBeenCalled();
  rootScan.mockRestore();
  expect(wrapper.getAttribute("tabindex")).toBe("0");
});

it("leaves table text changes to ResizeObserver rather than measuring on each streamed chunk", async () => {
  const wrapper = table();
  const reads = readsFor(wrapper);
  document.body.append(wrapper);
  dispose = installMarkdownTableFocus(document);
  reads.clear();
  const cell = wrapper.querySelector("td");
  cell.firstChild.data += " streamed";
  cell.append(document.createTextNode(" text"));
  cell.innerHTML = "<strong>More text</strong>";
  await settle();
  reads.expectNone();
  resize([{ target: wrapper.querySelector("table") }]);
  expect(reads.client).toHaveBeenCalledTimes(1);
  expect(reads.scroll).toHaveBeenCalledTimes(1);
});

it("measures only newly inserted table wrappers and keeps unchanged tables observed", async () => {
  const existing = table();
  const existingReads = readsFor(existing);
  document.body.append(existing);
  dispose = installMarkdownTableFocus(document);
  existingReads.clear();
  const added = table();
  const addedReads = readsFor(added);
  const repaint = document.createElement("section");
  repaint.append(added);
  document.body.append(repaint);
  await settle();
  existingReads.expectNone();
  expect(addedReads.client).toHaveBeenCalledTimes(1);
  expect(addedReads.scroll).toHaveBeenCalledTimes(1);
  expect(added.getAttribute("tabindex")).toBe("0");
  expect(observed.has(existing.querySelector("table"))).toBe(true);
  expect(observed.has(added.querySelector("table"))).toBe(true);
});

it("releases a removed wrapper without remeasuring other tables and observes its replacement", async () => {
  const retained = table();
  const removed = table();
  const retainedReads = readsFor(retained);
  const removedReads = readsFor(removed);
  document.body.append(retained, removed);
  dispose = installMarkdownTableFocus(document);
  retainedReads.clear();
  removedReads.clear();
  const previousTable = removed.querySelector("table");
  const replacement = table();
  removed.replaceWith(replacement);
  await settle();
  retainedReads.expectNone();
  removedReads.expectNone();
  expect(observed.has(removed)).toBe(false);
  expect(observed.has(previousTable)).toBe(false);
  expect(observed.has(replacement)).toBe(true);
  expect(observed.has(replacement.querySelector("table"))).toBe(true);
  expect(removed.hasAttribute("tabindex")).toBe(false);
});

it("does not retain or measure a new table when disposed before mutation delivery", async () => {
  dispose = installMarkdownTableFocus(document);
  const wrapper = table();
  const reads = readsFor(wrapper);
  document.body.append(wrapper);
  dispose();
  await settle();
  reads.expectNone();
  expect(observed.size).toBe(0);
  expect(wrapper.hasAttribute("tabindex")).toBe(false);
  dispose = null;
});

it("preserves an existing wrapper moved within one batch without measuring it again", async () => {
  const wrapper = table();
  const reads = readsFor(wrapper);
  const destination = document.createElement("section");
  document.body.append(wrapper, destination);
  dispose = installMarkdownTableFocus(document);
  reads.clear();
  wrapper.remove();
  destination.append(wrapper);
  await settle();
  reads.expectNone();
  expect(observed.size).toBe(2);
  expect(observed.has(wrapper)).toBe(true);
  expect(observed.has(wrapper.querySelector("table"))).toBe(true);
});

it("releases ownership when only the table is removed and ignores its stale resize delivery", async () => {
  const wrapper = table();
  const reads = readsFor(wrapper);
  document.body.append(wrapper);
  dispose = installMarkdownTableFocus(document);
  reads.clear();
  const original = wrapper.querySelector("table");
  original.remove();
  await settle();
  reads.expectNone();
  expect(observed.size).toBe(0);
  expect(wrapper.hasAttribute("tabindex")).toBe(false);
  resize([{ target: wrapper }, { target: original }]);
  reads.expectNone();
  wrapper.append(document.createElement("table"));
  await settle();
  expect(observed.size).toBe(2);
  expect(reads.client).toHaveBeenCalledTimes(1);
  expect(reads.scroll).toHaveBeenCalledTimes(1);
});

it("preserves table observation when a new wrapper receives an already observed table in the same batch", async () => {
  const original = table();
  document.body.append(original);
  dispose = installMarkdownTableFocus(document);
  const moved = original.querySelector("table");
  const destination = document.createElement("div");
  destination.className = "mdtable";
  const reads = readsFor(destination);
  document.body.append(destination);
  destination.append(moved);
  await settle();
  expect(observed.has(original)).toBe(false);
  expect(observed.has(destination)).toBe(true);
  expect(observed.has(moved)).toBe(true);
  reads.clear();
  resize([{ target: moved }]);
  expect(reads.client).toHaveBeenCalledTimes(1);
  expect(reads.scroll).toHaveBeenCalledTimes(1);
  moved.remove();
  await settle();
  expect(observed.size).toBe(0);
  expect(destination.hasAttribute("tabindex")).toBe(false);
});
