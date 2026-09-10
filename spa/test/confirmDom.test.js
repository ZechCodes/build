// @vitest-environment jsdom
// DOM semantics of confirmAction (the reusable modal confirm, G1): resolve
// values for Confirm / Cancel / Escape / scrim click, capture-phase Escape
// containment (an underlying sheet's handler must never see the press), and
// the safe default focus on Cancel.

import { describe, it, expect, beforeEach } from "vitest";
import { confirmAction, isConfirmOpen } from "../src/core/confirm.js";

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
