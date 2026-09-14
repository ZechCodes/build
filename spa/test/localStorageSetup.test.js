// Every suite has a localStorage, whatever Node the tree is run on.
//
// Node 25 shipped an experimental Web Storage implementation, and it installs
// `localStorage` on globalThis as an accessor pair that answers `undefined`
// unless the process was started with `--localstorage-file`. vitest's jsdom
// environment leaves a global that already exists alone, so jsdom's own Storage
// never lands and `localStorage.getItem` throws on the first line of
// src/app.js. A node-environment suite has never had one at all.
//
// test/setup/localStorage.js fills that gap for both. This suite runs in the
// node environment — the one with no Storage of its own — and pins what the
// setup file installs; the jsdom suites pin the other half by reading the
// browser preferences on every mount.

import { describe, it, expect, beforeEach } from "vitest";

describe("the localStorage every suite is given", () => {
  beforeEach(() => localStorage.clear());

  it("is there in a node-environment suite", () => {
    expect(typeof localStorage).toBe("object");
  });

  it("round-trips a value and says nothing for a name it was never given", () => {
    localStorage.setItem("build.selectedDeviceId", "dev-1");
    expect(localStorage.getItem("build.selectedDeviceId")).toBe("dev-1");
    expect(localStorage.getItem("build.nothingHere")).toBeNull();
  });

  it("stores what it was given as text, the way a browser does", () => {
    localStorage.setItem("build.count", 7);
    expect(localStorage.getItem("build.count")).toBe("7");
  });

  it("forgets one name, and forgets all of them", () => {
    localStorage.setItem("a", "1");
    localStorage.setItem("b", "2");
    localStorage.removeItem("a");
    expect(localStorage.getItem("a")).toBeNull();
    expect(localStorage.length).toBe(1);
    expect(localStorage.key(0)).toBe("b");
    localStorage.clear();
    expect(localStorage.length).toBe(0);
  });
});
