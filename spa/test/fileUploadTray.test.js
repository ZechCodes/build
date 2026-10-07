// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { mountFileUploadTray } from "../src/core/fileUploadTray.js";

afterEach(() => { vi.useRealTimers(); document.body.replaceChildren(); });

it("hides an empty tray, subscribes to state, prunes recent history and disposes", () => {
  vi.useFakeTimers();
  let snapshot = { active: [], recent: [] };
  let listener;
  const unsubscribe = vi.fn();
  const uploads = { snapshot: () => snapshot, subscribe: (fn) => { listener = fn; return unsubscribe; }, prune: vi.fn(), cancel: vi.fn(), retry: vi.fn() };
  const dispose = mountFileUploadTray(document.body, { uploads });
  expect(document.querySelector(".fupload-tray").hidden).toBe(true);
  snapshot = { active: [{ id: "one", name: "one.txt", size: 100, received: 25, status: "uploading" }], recent: [] };
  listener();
  expect(document.querySelector('[role="status"]').textContent).toBe("Uploading 1 file · 25%");
  document.querySelector(".fupload-summary").click();
  document.querySelector(".fupload-action").click();
  expect(uploads.cancel).toHaveBeenCalledWith("one");
  vi.advanceTimersByTime(60_000);
  expect(uploads.prune).toHaveBeenCalledTimes(2);
  dispose();
  expect(unsubscribe).toHaveBeenCalledOnce();
  expect(document.querySelector(".fupload-tray")).toBeNull();
  vi.advanceTimersByTime(60_000);
  expect(uploads.prune).toHaveBeenCalledTimes(2);
});

it("announces one finished upload in the idle summary", () => {
  const uploads = { snapshot: () => ({ active: [], recent: [{ id: "one", status: "finished", name: "one.txt", finishedAt: Date.now() }] }), subscribe: () => () => {}, prune() {} };
  const dispose = mountFileUploadTray(document.body, { uploads });
  expect(document.querySelector('[role="status"]').textContent).toBe("1 upload finished");
  dispose();
});

it("formats large transfer totals in readable units", () => {
  const uploads = { snapshot: () => ({ active: [{ id: "one", name: "movie.mp4", size: 2000000000, received: 500000000, status: "uploading" }], recent: [] }), subscribe: () => () => {}, prune() {} };
  const dispose = mountFileUploadTray(document.body, { uploads });
  document.querySelector(".fupload-summary").click();
  expect(document.querySelector(".fupload-detail").textContent).toBe("500 MB / 2.0 GB");
  dispose();
});

it("keeps keyboard focus on an action while progress updates", () => {
  let listener;
  const item = { id: "one", name: "one.txt", size: 100, received: 20, status: "uploading" };
  const uploads = { snapshot: () => ({ active: [item], recent: [] }), subscribe: (fn) => { listener = fn; return () => {}; }, prune() {}, cancel() {} };
  const dispose = mountFileUploadTray(document.body, { uploads });
  document.querySelector(".fupload-summary").click();
  document.querySelector(".fupload-action").focus();
  item.received = 30;
  listener();
  expect(document.activeElement.getAttribute("aria-label")).toBe("Cancel one.txt");
  dispose();
});

it("gates recent Retry and Replace from cached support while keeping active Cancel", () => {
  const uploads = { snapshot: () => ({ active: [{ id: "active", name: "active.txt", size: 100, received: 0, status: "uploading" }], recent: [{ id: "failed", name: "existing.txt", status: "failed", errorCode: "already_exists", canRetry: true, finishedAt: Date.now() }] }), subscribe: () => () => {}, prune() {}, cancel() {}, retry() {} };
  const dispose = mountFileUploadTray(document.body, { uploads, capabilities: { uploads: false } });
  document.querySelector(".fupload-summary").click();
  document.querySelector(".fupload-recent-toggle").click();
  expect([...document.querySelectorAll(".fupload-action")].map((node) => node.textContent)).toEqual(["Cancel"]);
  dispose.setCapabilities({ uploads: true });
  expect([...document.querySelectorAll(".fupload-action")].map((node) => node.textContent)).toEqual(["Cancel", "Retry", "Replace"]);
  dispose();
});
