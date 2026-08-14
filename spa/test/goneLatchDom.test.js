// @vitest-environment jsdom
// The dead-route gone-latch (item 6): an "unknown plan_id"/"unknown run_id"
// from the first fetch latches a terminal "no longer exists" state, stops
// repainting (no further RPCs from paint), and offers a way back.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { App } from "../src/app.js";
import { renderPlan } from "../src/views/plan.js";
import { renderTask } from "../src/views/task.js";
import { TerminalSocket } from "../src/terminal/session.js";

function installGoneRpc(goneMessage) {
  const calls = [];
  App.call = (method, params) => {
    calls.push(method);
    return Promise.reject(new Error(goneMessage));
  };
  return calls;
}

describe("gone-latch (DOM)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = '<div id="root"></div>';
    App.offline = false;
    App.poll = null;
    App.viewDispose = null;
    // The task view's terminal tabs ride a real WebSocket in production; keep
    // the singleton socket inert and empty here.
    vi.spyOn(TerminalSocket.prototype, "start").mockResolvedValue();
    vi.spyOn(TerminalSocket.prototype, "listTerminals").mockResolvedValue([]);
  });
  afterEach(() => {
    if (App.poll) clearInterval(App.poll);
    App.poll = null;
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("plan: latches the gone state, stops fetching, and routes back to the inbox", async () => {
    const calls = installGoneRpc("unknown plan_id: p-gone");
    App.route = { name: "plan", id: "p-gone", tab: "review" };
    location.hash = "#/plan/p-gone/review";
    await renderPlan();
    await vi.advanceTimersByTimeAsync(0);

    const root = document.getElementById("root");
    expect(root.textContent).toContain("This Issue no longer exists");
    expect(root.textContent).toContain("Back to notifications"); // no project learned

    // The latch freezes the view: later ticks fetch nothing and never repaint.
    const fetchesAtLatch = calls.filter((m) => m === "issue.get").length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(calls.filter((m) => m === "issue.get").length).toBe(fetchesAtLatch);
    expect(root.textContent).toContain("This Issue no longer exists");

    document.getElementById("goneback").click();
    // The way out of a gone entity is the landing surface — the inbox.
    expect(location.hash).toBe("#/inbox");
  });

  it("task: latches the gone state and stops the poll", async () => {
    const calls = installGoneRpc("unknown run_id: r-gone");
    App.route = { name: "task", id: "r-gone", tab: "stages" };
    location.hash = "#/task/r-gone/stages";
    await renderTask();
    await vi.advanceTimersByTimeAsync(0);

    const root = document.getElementById("root");
    expect(root.textContent).toContain("This task no longer exists");

    const fetchesAtLatch = calls.filter((m) => m === "run.get").length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(calls.filter((m) => m === "run.get").length).toBe(fetchesAtLatch);
    expect(root.textContent).toContain("This task no longer exists");

    document.getElementById("goneback").click();
    // The way out of a gone entity is the landing surface — the inbox.
    expect(location.hash).toBe("#/inbox");
  });

  it("a transient error does NOT latch — the poll keeps retrying", async () => {
    const calls = installGoneRpc("relay timeout");
    App.route = { name: "plan", id: "p-flaky", tab: "review" };
    location.hash = "#/plan/p-flaky/review";
    await renderPlan();
    await vi.advanceTimersByTimeAsync(0);

    const root = document.getElementById("root");
    expect(root.textContent).not.toContain("no longer exists");

    const fetchesAtStart = calls.filter((m) => m === "issue.get").length;
    await vi.advanceTimersByTimeAsync(3300); // two poll ticks
    expect(calls.filter((m) => m === "issue.get").length).toBeGreaterThan(fetchesAtStart);
  });
});
