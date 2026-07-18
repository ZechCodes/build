// @vitest-environment jsdom
// DOM wiring for mountSplitButton: single-flight while run() is awaiting —
// primary + caret disabled, menu invokes ignored — and the shared-flight form
// (a remount mid-flight keeps the latch, so no concurrent destructive RPCs).

import { describe, it, expect } from "vitest";
import { mountSplitButton, createSingleFlight } from "../src/core/splitButton.js";

const OPTIONS = [
  { id: "merge_prune", label: "Merge", menuLabel: "Merge & clean up", description: "d", busyLabel: "merging…" },
  { id: "commit", menuLabel: "Commit", description: "d", busyLabel: "committing…" },
];

function pendingRun() {
  const calls = [];
  let settle;
  const gate = new Promise((resolve) => (settle = resolve));
  const run = (optionId) => {
    calls.push(optionId);
    return gate;
  };
  return { run, calls, settle };
}

function mount(runSpec, flight) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  mountSplitButton(container, { options: OPTIONS, run: runSpec.run, flight });
  return {
    container,
    primary: container.querySelector(".btn.primary:not(.caret)"),
    caret: container.querySelector(".caret"),
    menu: container.querySelector(".splitmenu"),
  };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("mountSplitButton in-flight guard (DOM)", () => {
  it("disables primary + caret and shows the busy label while run() is pending", async () => {
    const spec = pendingRun();
    const { primary, caret } = mount(spec);
    primary.click();
    expect(primary.disabled).toBe(true);
    expect(caret.disabled).toBe(true);
    expect(primary.textContent).toBe("merging…");
    spec.settle();
    await tick();
    expect(spec.calls).toEqual(["merge_prune"]);
  });

  it("ignores menu invokes while in flight (single flight)", async () => {
    const spec = pendingRun();
    const { primary, menu } = mount(spec);
    primary.click();
    menu.querySelector('[data-action="commit"]').click();
    expect(spec.calls).toEqual(["merge_prune"]);
    spec.settle();
    await tick();
  });

  it("a rejection restores label + enabled so the user can retry", async () => {
    const calls = [];
    const run = (optionId) => {
      calls.push(optionId);
      return Promise.reject(new Error("boom"));
    };
    const container = document.createElement("div");
    document.body.appendChild(container);
    mountSplitButton(container, { options: OPTIONS, run });
    const primary = container.querySelector(".btn.primary:not(.caret)");
    primary.click();
    await tick();
    expect(primary.disabled).toBe(false);
    expect(primary.textContent).toBe("Merge");
    primary.click();
    await tick();
    expect(calls).toEqual(["merge_prune", "merge_prune"]);
  });

  it("a shared external flight blocks invokes on a remounted button mid-flight", async () => {
    const flight = createSingleFlight();
    const first = pendingRun();
    const a = mount(first, flight);
    a.primary.click();
    expect(first.calls).toEqual(["merge_prune"]);

    // A poll tick remounts the button while the RPC is still in flight: the
    // fresh button shares the latch, so a click dispatches nothing.
    const second = pendingRun();
    const b = mount(second, flight);
    b.primary.click();
    b.menu.querySelector('[data-action="commit"]').click();
    expect(second.calls).toEqual([]);

    // Once the original flight settles, the latch re-arms.
    first.settle();
    await tick();
    b.primary.click();
    expect(second.calls).toEqual(["merge_prune"]);
  });
});
