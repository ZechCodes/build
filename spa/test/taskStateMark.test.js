// @vitest-environment jsdom
// The mark a task wears on its board card and at the head of its page: a
// checkbox, where the old dot was GitHub's circle (#190). Empty while
// the task is open, checked once it is done, slashed when it was closed
// without being done — not planned.
import { describe, expect, it } from "vitest";
import { stateMarkHtml } from "../src/core/trackerChips.js";
import { taskHeadHtml } from "../src/core/trackerTaskRender.js";

const mark = (task) => {
  const host = document.createElement("div");
  host.innerHTML = stateMarkHtml(task);
  return host.firstElementChild;
};

describe("a task's state mark", () => {
  it("is an empty box while the task is open", () => {
    const open = mark({ state: "open", status: "in_progress" });
    expect(open.classList).toContain("task-state-open");
    expect(open.querySelector("svg").classList).toContain("lucide-square");
    expect(open.getAttribute("aria-label")).toMatch(/^Open\. /);
  });

  it("is a checked box once the task is done, closed or not", () => {
    for (const state of ["open", "closed"]) {
      const done = mark({ state, status: "done" });
      expect(done.classList).toContain("task-state-done");
      expect(done.querySelector("svg").classList).toContain("lucide-square-check");
      expect(done.getAttribute("aria-label")).toMatch(/^Done\. /);
    }
  });

  it("is a slashed box for a task closed without being done", () => {
    const closed = mark({ state: "closed", status: "backlog" });
    expect(closed.classList).toContain("task-state-closed");
    expect(closed.querySelector("svg").classList).toContain("lucide-square-slash");
    expect(closed.getAttribute("aria-label")).toMatch(/^Closed\. /);
  });

  it("is never a circle", () => {
    for (const task of [{ state: "open" }, { state: "open", status: "done" }, { state: "closed" }]) {
      expect(stateMarkHtml(task)).not.toContain("circle");
    }
  });

  // A checked box beside "Open" would contradict itself: the word at the head
  // of a task's page says what the mark beside it draws.
  it("is named beside it at the head of the task's page", () => {
    const said = (task) => {
      const host = document.createElement("div");
      host.innerHTML = taskHeadHtml({ title: "t", number: 1, updated_at: null, ...task });
      return host.querySelector(".task-page-state").textContent;
    };
    expect(said({ state: "open", status: "ready" })).toBe("Open");
    expect(said({ state: "open", status: "done" })).toBe("Done");
    expect(said({ state: "closed", status: "backlog" })).toBe("Closed");
  });
});
