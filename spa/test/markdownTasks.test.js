/** @vitest-environment jsdom */
import { describe, expect, it } from "vitest";

import { markdownHtml } from "../src/core/markdown.js";
import { setMarkdownTaskChecked } from "../src/core/markdownTasks.js";

function checkboxes(source, options = {}) {
  const host = document.createElement("div");
  host.innerHTML = markdownHtml(source, options);
  return [...host.querySelectorAll('input[type="checkbox"]')];
}

describe("markdown checklist items", () => {
  it("renders unchecked and checked markers in ordered and unordered lists", () => {
    const inputs = checkboxes("- [ ] Ship\n- [x] Test\n- [X] Review\n\n3) [ ] Deploy");
    expect(inputs).toHaveLength(4);
    expect(inputs.map((input) => input.checked)).toEqual([false, true, true, false]);
    expect(inputs.map((input) => input.getAttribute("aria-label"))).toEqual(["Ship", "Test", "Review", "Deploy"]);
    expect(inputs.map((input) => input.dataset.taskIndex)).toEqual(["0", "1", "2", "3"]);
    expect(inputs.every((input) => input.closest("li").classList.contains("task-item"))).toBe(true);
  });

  it("keeps every surface read-only until it opts into task interaction", () => {
    expect(checkboxes("- [ ] Ship")[0].disabled).toBe(true);
    expect(checkboxes("- [ ] Ship", { taskItems: true })[0].disabled).toBe(false);
    expect(checkboxes("- [x] Ship", { taskItems: false })[0].disabled).toBe(true);
  });

  it("does not invent controls in prose, code, tables or malformed markers", () => {
    const source = "[ ] Prose\n\n`- [ ] Inline`\n\n```md\n- [ ] Fence\n```\n\n| task |\n|---|\n| - [ ] Cell |\n\n- [no] Wrong\n- [ ]nospace\n- \\[ ] Escaped";
    expect(checkboxes(source, { taskItems: true })).toEqual([]);
    expect(checkboxes("- [ ] Ship", { mode: "inline", taskItems: true })).toEqual([]);
    expect(markdownHtml("- [ ] Ship", { mode: "plain", taskItems: true })).not.toContain("<input");
  });

  it("supports empty items and tab-separated labels without duplicate ids", () => {
    const inputs = checkboxes("- [ ]\n- [x]\tDone", { taskItems: true });
    expect(inputs).toHaveLength(2);
    expect(inputs[0].getAttribute("aria-label")).toBe("Checklist item 1");
    expect(inputs[1].getAttribute("aria-label")).toBe("Done");
    expect(inputs.every((input) => !input.hasAttribute("id") && !input.hasAttribute("tabindex"))).toBe(true);
  });

  it("keeps inline formatting, continuation paragraphs and nested blocks", () => {
    const source = "- [ ] **Ship** `release`\n  still the same item\n\n  a later paragraph\n  > quoted\n  - [x] Test";
    const inputs = checkboxes(source);
    expect(inputs).toHaveLength(2);
    const parent = inputs[0].closest("li");
    expect(parent.querySelector("strong").textContent).toBe("Ship");
    expect(parent.querySelector("code").textContent).toBe("release");
    expect(parent.textContent).toContain("still the same item");
    expect(parent.querySelector("p").textContent).toBe("a later paragraph");
    expect(parent.querySelector("blockquote").textContent).toBe("quoted");
    expect(inputs[1].closest("li").parentElement.parentElement).toBe(parent);
  });

  it("assigns original checked-character offsets through quotes, lists and CRLF", () => {
    const source = "Intro\r\n\r\n> - [ ] Parent\r\n>   2. [X] Child\r\n>      > + [x] Quoted\r\n\r\n- [ ] Last";
    const offsets = [...source.matchAll(/\[([ xX])\]/g)].map((match) => match.index + 1);
    const markers = [];
    const inputs = checkboxes(source, { taskItems: true, taskMarkers: markers });
    expect(inputs.map((input) => Number(input.dataset.taskOffset))).toEqual(offsets);
    expect(markers).toEqual(offsets);
    expect(inputs.map((input) => input.dataset.taskIndex)).toEqual(["0", "1", "2", "3"]);
  });

  it("does not read checklist markers past the renderer's nesting bound", () => {
    const source = `${"> ".repeat(8)}- [ ] Too deep\n\n- [ ] Visible`;
    const inputs = checkboxes(source);
    expect(inputs).toHaveLength(1);
    expect(inputs[0].dataset.taskIndex).toBe("0");
    expect(inputs[0].dataset.taskOffset).toBe(String(source.lastIndexOf("[ ]") + 1));
  });
});

describe("ticking the source markdown", () => {
  it("rewrites only the requested marker's checked character", () => {
    const source = "- [ ] Same **label**\n- [ ] Same **label**\n- [X] Keep uppercase\n";
    expect(setMarkdownTaskChecked(source, 1, true)).toBe("- [ ] Same **label**\n- [x] Same **label**\n- [X] Keep uppercase\n");
    expect(setMarkdownTaskChecked(source, 2, false)).toBe("- [ ] Same **label**\n- [ ] Same **label**\n- [ ] Keep uppercase\n");
    expect(setMarkdownTaskChecked(source, 2, true)).toBe(source);
    expect(setMarkdownTaskChecked(source, 0, false)).toBe(source);
  });

  it("follows the renderer's item order while preserving nested source and CRLF", () => {
    const source = "```\r\n- [ ] Ignore\r\n```\r\n> - [ ] Parent\r\n>   - [ ] Child\r\n>     > 1. [X] Quoted\r\n\r\n- [ ] Last\r\n";
    const marker = source.indexOf("[X]") + 1;
    expect(setMarkdownTaskChecked(source, 2, false)).toBe(source.slice(0, marker) + " " + source.slice(marker + 1));
    const parent = source.indexOf("[ ] Parent") + 1;
    expect(setMarkdownTaskChecked(source, 0, true)).toBe(source.slice(0, parent) + "x" + source.slice(parent + 1));
  });

  it("skips fences, table cells, inline code and over-depth list prose", () => {
    const source = "```\n- [ ] Fence\n```\n\n| h |\n|---|\n| - [ ] Table |\n\n`- [ ] Inline`\n\n" + `${"> ".repeat(8)}- [ ] Deep\n\n- [ ] Visible`;
    expect(setMarkdownTaskChecked(source, 0, true)).toBe(source.replace("[ ] Visible", "[x] Visible"));
    expect(setMarkdownTaskChecked(source, 1, true)).toBeNull();
  });

  it("rejects invalid indexes or states without changing the source", () => {
    for (const index of [-1, 0.5, 1, NaN, "0", null]) expect(setMarkdownTaskChecked("- [ ] Ship", index, true)).toBeNull();
    for (const checked of ["true", 1, null, undefined]) expect(setMarkdownTaskChecked("- [ ] Ship", 0, checked)).toBeNull();
    expect(setMarkdownTaskChecked("No checklist", 0, true)).toBeNull();
  });
});
