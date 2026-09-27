// @vitest-environment jsdom
// The mark a task wears on its board card and at the head of its page: a
// checkbox, where the old dot was GitHub's circle (#190). Its shape says how
// far the work got — empty while the task is open, checked once it is done,
// slashed when it was closed without being done (not planned). Its colour says
// whether the task is open or closed, whatever the shape: open/closed and the
// Done column move independently, and both are always shown.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { stateMarkHtml } from "../src/core/trackerChips.js";
import { taskHeadHtml } from "../src/core/trackerTaskRender.js";

const mark = (task) => {
  const host = document.createElement("div");
  host.innerHTML = stateMarkHtml(task);
  return host.firstElementChild;
};

const tasksCss = readFileSync(resolve("src/styles/tasks.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const rules = [...tasksCss.matchAll(/([^{}]+)\{([^}]*)\}/g)].map(([, selector, body]) => [selector.trim(), body]);
const colourOf = (selector) => {
  const [, body] = rules.find(([name]) => name === selector) || [, ""];
  return body.match(/(?:^|[\s;])color:\s*([^;]+)/)?.[1].trim();
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
      expect(done.classList).toContain("task-mark-done");
      expect(done.querySelector("svg").classList).toContain("lucide-square-check");
    }
  });

  it("is a slashed box for a task closed without being done", () => {
    const closed = mark({ state: "closed", status: "backlog" });
    expect(closed.classList).toContain("task-state-closed");
    expect(closed.querySelector("svg").classList).toContain("lucide-square-slash");
    expect(closed.getAttribute("aria-label")).toMatch(/^Closed\. /);
  });

  // A closed task in Done used to be violet, like every closed task; the check
  // must not make it read as open.
  it("wears the closed colour when closed, whatever its shape", () => {
    const closedDone = mark({ state: "closed", status: "done" });
    expect(closedDone.classList).toContain("task-state-closed");
    expect(closedDone.classList).not.toContain("task-state-open");
    expect(closedDone.getAttribute("aria-label")).toMatch(/^Closed, done\. /);
    const openDone = mark({ state: "open", status: "done" });
    expect(openDone.classList).toContain("task-state-open");
    expect(openDone.getAttribute("aria-label")).toMatch(/^Open, done\. /);
  });

  it("is green while open and violet once closed", () => {
    expect(colourOf(".task-state")).toBe("var(--green)");
    expect(colourOf(".task-state-closed")).toBe("var(--violet)");
    expect(tasksCss).not.toMatch(/\.task-mark-[a-z]+ \{[^}]*color:/);
  });

  it("is never a circle", () => {
    for (const task of [{ state: "open" }, { state: "open", status: "done" }, { state: "closed" }]) {
      expect(stateMarkHtml(task)).not.toContain("circle");
    }
  });

  // "Done" is carried by the check and the column chip; the word at the head of
  // a task's page says the other fact, open or closed.
  it("is named Open or Closed at the head of the task's page", () => {
    const said = (task) => {
      const host = document.createElement("div");
      host.innerHTML = taskHeadHtml({ title: "t", number: 1, updated_at: null, ...task });
      return host.querySelector(".task-page-state").textContent;
    };
    expect(said({ state: "open", status: "ready" })).toBe("Open");
    expect(said({ state: "open", status: "done" })).toBe("Open");
    expect(said({ state: "closed", status: "done" })).toBe("Closed");
    expect(said({ state: "closed", status: "backlog" })).toBe("Closed");
  });
});
