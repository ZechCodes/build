// @vitest-environment jsdom
// DOM semantics of confirmAction (the reusable modal confirm, G1): resolve
// values for Confirm / Cancel / Escape / scrim click, capture-phase Escape
// containment (an underlying sheet's handler must never see the press), and
// the safe default focus on Cancel.

import { describe, it, expect, beforeEach } from "vitest";
import { confirmAction, confirmActionAt, isConfirmOpen } from "../src/core/confirm.js";

const OPTS = { title: "Merge feat/x?", actions: ["Commit", "Merge", "Delete branch"], confirmLabel: "Merge", danger: true };

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("confirmAction (DOM)", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("resolves true on Confirm and removes the scrim", async () => {
    const pending = confirmAction(OPTS);
    expect(isConfirmOpen()).toBe(true);
    document.querySelector("[data-confirm-ok]").click();
    await expect(pending).resolves.toBe(true);
    expect(isConfirmOpen()).toBe(false);
  });

  it("resolves false on Cancel", async () => {
    const pending = confirmAction(OPTS);
    document.querySelector("[data-confirm-cancel]").click();
    await expect(pending).resolves.toBe(false);
    expect(isConfirmOpen()).toBe(false);
  });

  it("focuses Cancel on open (safe default for destructive verbs)", () => {
    const pending = confirmAction(OPTS);
    expect(document.activeElement).toBe(document.querySelector("[data-confirm-cancel]"));
    document.querySelector("[data-confirm-cancel]").click();
    return pending;
  });

  it("Escape cancels at capture phase and never reaches an underlying handler", async () => {
    let underlyingSawEscape = false;
    const underlying = (event) => {
      if (event.key === "Escape") underlyingSawEscape = true;
    };
    document.addEventListener("keydown", underlying); // bubble phase, like a sheet's handler
    const pending = confirmAction(OPTS);
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await expect(pending).resolves.toBe(false);
    expect(underlyingSawEscape).toBe(false);
    expect(isConfirmOpen()).toBe(false);
    document.removeEventListener("keydown", underlying);

    // The capture listener is gone after settle: Escape now propagates again.
    document.addEventListener("keydown", underlying);
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(underlyingSawEscape).toBe(true);
    document.removeEventListener("keydown", underlying);
  });

  it("a click on the scrim cancels; a click inside the modal does not settle", async () => {
    let settled = null;
    const pending = confirmAction(OPTS).then((v) => (settled = v));
    document.querySelector(".modal").click(); // inside — must not settle
    await flush();
    expect(settled).toBeNull();
    expect(isConfirmOpen()).toBe(true);
    document.getElementById("confirm-scrim").click(); // the scrim itself
    await pending;
    expect(settled).toBe(false);
  });
});

describe("confirmActionAt (DOM)", () => {
  beforeEach(() => {
    document.body.innerHTML = '<button id="done">Done</button>';
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 320 });
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 240 });
  });

  it("opens below its anchor without the global confirmation scrim", async () => {
    const anchor = document.getElementById("done");
    anchor.getBoundingClientRect = () => ({ left: 280, right: 320, top: 20, bottom: 48, width: 40, height: 28 });
    const pending = confirmActionAt(anchor, OPTS);
    const popover = document.querySelector(".confirm-popover");

    expect(document.getElementById("confirm-scrim")).toBeNull();
    expect(popover.getAttribute("role")).toBe("dialog");
    expect(popover.style.top).toBe("54px");
    expect(popover.style.maxHeight).toBe("178px");
    expect(Number.parseFloat(popover.style.left)).toBeLessThan(280);
    expect(document.activeElement).toBe(popover.querySelector("[data-confirm-cancel]"));

    popover.querySelector("[data-confirm-cancel]").click();
    await expect(pending).resolves.toBe(false);
    expect(document.activeElement).toBe(anchor);
  });

  it("cancels on an outside press and Escape without reaching underlying handlers", async () => {
    const anchor = document.getElementById("done");
    anchor.getBoundingClientRect = () => ({ left: 20, right: 60, top: 20, bottom: 48, width: 40, height: 28 });
    let underlyingSawEscape = false;
    const underlying = (event) => { if (event.key === "Escape") underlyingSawEscape = true; };
    document.addEventListener("keydown", underlying);

    const escaped = confirmActionAt(anchor, OPTS);
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await expect(escaped).resolves.toBe(false);
    expect(underlyingSawEscape).toBe(false);

    const outside = confirmActionAt(anchor, OPTS);
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    await expect(outside).resolves.toBe(false);
    document.removeEventListener("keydown", underlying);
  });

  it("cancels when repainting removes the anchor", async () => {
    const anchor = document.getElementById("done");
    anchor.getBoundingClientRect = () => ({ left: 20, right: 60, top: 20, bottom: 48, width: 40, height: 28 });
    const pending = confirmActionAt(anchor, OPTS);
    anchor.remove();
    await new Promise((resolve) => setTimeout(resolve, 0));
    await expect(pending).resolves.toBe(false);
    expect(document.querySelector(".confirm-popover")).toBeNull();
  });

  it.each(["resize", "scroll"])("cancels when the viewport emits %s", async (type) => {
    const anchor = document.getElementById("done");
    anchor.getBoundingClientRect = () => ({ left: 20, right: 60, top: 20, bottom: 48, width: 40, height: 28 });
    const pending = confirmActionAt(anchor, OPTS);
    const target = type === "resize" ? window : document;
    target.dispatchEvent(new Event(type));
    await expect(pending).resolves.toBe(false);
    expect(document.querySelector(".confirm-popover")).toBeNull();
  });

  it("stays open when an unrelated sibling scrolls", async () => {
    const anchor = document.getElementById("done");
    anchor.getBoundingClientRect = () => ({ left: 20, right: 60, top: 20, bottom: 48, width: 40, height: 28 });
    const sibling = document.body.appendChild(document.createElement("div"));
    const pending = confirmActionAt(anchor, OPTS);
    sibling.dispatchEvent(new Event("scroll", { bubbles: false }));
    expect(document.querySelector(".confirm-popover")).toBeTruthy();
    document.querySelector("[data-confirm-cancel]").click();
    await expect(pending).resolves.toBe(false);
  });
});
