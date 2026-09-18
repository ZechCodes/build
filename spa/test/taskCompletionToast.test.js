// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountTaskCompletionToast } from "../src/core/taskCompletionToast.js";

describe("task completion toast", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = '<div id="rail"><button data-bubble="agent" data-agent="ag-1"></button></div>';
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1000 });
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 700 });
    document.querySelector("button").getBoundingClientRect = () => ({ left: 900, top: 100, width: 32, height: 32 });
  });
  afterEach(() => vi.useRealTimers());

  it("is a temporary polite status placed left of a desktop bubble", () => {
    const mounted = mountTaskCompletionToast(document.querySelector("#rail"));
    mounted.show("ag-1", ["Ship the fix"]);
    const toast = document.querySelector(".task-completion-toast");
    toast.getBoundingClientRect = () => ({ width: 180, height: 36 });
    window.dispatchEvent(new Event("resize"));
    expect(toast.getAttribute("role")).toBe("status");
    expect(toast.textContent).toBe("Task completed: Ship the fix");
    expect(parseFloat(toast.style.left)).toBeLessThan(900);
    vi.advanceTimersByTime(4000);
    expect(document.querySelector(".task-completion-toast")).toBe(null);
    mounted.dispose();
  });

  it("places grouped completions above a mobile bubble and clamps the viewport", () => {
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 360 });
    document.querySelector("button").getBoundingClientRect = () => ({ left: 4, top: 620, width: 32, height: 32 });
    const mounted = mountTaskCompletionToast(document.querySelector("#rail"));
    mounted.show("ag-1", ["One", "Two"]);
    const toast = document.querySelector(".task-completion-toast");
    toast.getBoundingClientRect = () => ({ width: 240, height: 40 });
    window.dispatchEvent(new Event("resize"));
    expect(toast.textContent).toBe("2 tasks completed: One, Two");
    expect(parseFloat(toast.style.top)).toBeLessThan(620);
    expect(parseFloat(toast.style.left)).toBe(10);
    mounted.dispose();
  });

  it("queues agents and clears the queue and active timer on disposal", () => {
    document.querySelector("#rail").insertAdjacentHTML("beforeend", '<button data-bubble="agent" data-agent="ag-2"></button>');
    document.querySelector('[data-agent="ag-2"]').getBoundingClientRect = () => ({ left: 850, top: 150, width: 32, height: 32 });
    const mounted = mountTaskCompletionToast(document.querySelector("#rail"));
    mounted.show("ag-1", ["One"]);
    mounted.show("ag-2", ["Two"]);
    expect(document.querySelector(".task-completion-toast").textContent).toContain("One");
    vi.advanceTimersByTime(4000);
    expect(document.querySelector(".task-completion-toast").textContent).toContain("Two");
    mounted.dispose();
    vi.runAllTimers();
    expect(document.querySelector(".task-completion-toast")).toBe(null);
  });

  it("skips an offscreen queued bubble and advances to the next agent", () => {
    document.querySelector("button").getBoundingClientRect = () => ({ left: -50, right: -18, top: 100, bottom: 132, width: 32, height: 32 });
    document.querySelector("#rail").insertAdjacentHTML("beforeend", '<button data-bubble="agent" data-agent="ag-2"></button>');
    document.querySelector('[data-agent="ag-2"]').getBoundingClientRect = () => ({ left: 850, right: 882, top: 150, bottom: 182, width: 32, height: 32 });
    const mounted = mountTaskCompletionToast(document.querySelector("#rail"));
    mounted.show("ag-1", ["Hidden"]);
    mounted.show("ag-2", ["Visible"]);
    expect(document.querySelector(".task-completion-toast")?.textContent).toContain("Visible");
    mounted.dispose();
  });
});
