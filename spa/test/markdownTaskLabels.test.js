/** @vitest-environment jsdom */
import { afterEach, describe, expect, it } from "vitest";
import { markdownHtml } from "../src/core/markdown.js";
import { holdReferenceSources } from "../src/core/referenceIndex.js";
import { taskBodyHtml, timelineHtml } from "../src/core/trackerTaskRender.js";

const hostOf = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
};
const labelOf = (input) => input.getRootNode().querySelector(`[id="${input.getAttribute("aria-labelledby")}"]`);

afterEach(() => holdReferenceSources({}));

describe("rendered checklist names", () => {
  it("names a scoped checkbox from the rendered formatting and resolved reference", () => {
    holdReferenceSources({ tasks: { "d1/p1": [{ id: "task-1", number: 1, title: "Release notes" }] } });
    const host = hostOf(markdownHtml("- [ ] **Ship** `release` #1", {
      place: { deviceId: "d1", projectId: "p1" }, taskItems: true, taskLabelScope: "task-body:task-1",
    }));
    const input = host.querySelector("input");
    expect(input.hasAttribute("aria-label")).toBe(false);
    expect(labelOf(input)?.textContent).toBe("Ship release #1 Release notes");
    expect(labelOf(input).querySelector("strong").textContent).toBe("Ship");
    expect(labelOf(input).querySelector("code").textContent).toBe("release");
    expect(labelOf(input).querySelector("a.md-ref")).not.toBeNull();
  });

  it("keeps a parent's name separate from later paragraphs and child items", () => {
    const host = hostOf(markdownHtml("- [ ] **Ship**\n  the release\n\n  extra details\n  - [x] Test", { taskLabelScope: "nested" }));
    const inputs = [...host.querySelectorAll("input")];
    expect(labelOf(inputs[0])?.textContent).toBe("Ship the release");
    expect(labelOf(inputs[1])?.textContent).toBe("Test");
    expect(labelOf(inputs[0]).contains(inputs[1])).toBe(false);
    expect(inputs.every((input) => input.disabled)).toBe(true);
  });

  it("provides a rendered fallback name for an empty scoped item", () => {
    const host = hostOf(markdownHtml("- [ ]\n  - [x] Child", { taskLabelScope: "empty" }));
    const inputs = [...host.querySelectorAll("input")];
    expect(labelOf(inputs[0])?.textContent).toBe("Checklist item 1");
    expect(labelOf(inputs[0]).classList.contains("sr-only")).toBe(true);
    expect(labelOf(inputs[1])?.textContent).toBe("Child");
  });

  it("names a heading label without its nested child's text or control", () => {
    const host = hostOf(markdownHtml("- [ ] # Ship\n  - [x] Child", { taskLabelScope: "heading" }));
    const inputs = [...host.querySelectorAll("input")];
    expect(labelOf(inputs[0])?.textContent).toBe("Ship");
    expect(labelOf(inputs[0]).contains(inputs[1])).toBe(false);
    expect(host.querySelector("h1").textContent).toBe("Ship");
  });

  it("uses a continuation paragraph as an empty marker's rendered label", () => {
    const host = hostOf(markdownHtml("- [ ]\n  continuation description\n  - [x] Child", { taskLabelScope: "continued" }));
    const inputs = [...host.querySelectorAll("input")];
    expect(labelOf(inputs[0])?.textContent).toBe("continuation description");
    expect(labelOf(inputs[0]).classList.contains("sr-only")).toBe(false);
    expect(labelOf(inputs[0]).contains(inputs[1])).toBe(false);
  });

  it("separates label ids from heading anchors and safely encodes caller scopes", () => {
    const scopes = ["", "a-b", "a:b", '\"><svg onload="evil()">', "\u0000", "\ud800"];
    const html = scopes.map((taskLabelScope) => markdownHtml("- [ ] Label", { taskLabelScope })).join("");
    const host = hostOf(html);
    const ids = [...host.querySelectorAll("[id]")].map((element) => element.id);
    expect(new Set(ids).size).toBe(scopes.length);
    expect(host.querySelector("svg")).toBeNull();
    const alongsideHeading = hostOf(markdownHtml(`# ${ids[1]}\n\n- [ ] Label`, { taskLabelScope: scopes[1] }));
    const both = [...alongsideHeading.querySelectorAll("[id]")].map((element) => element.id);
    expect(new Set(both).size).toBe(2);
    expect(labelOf(alongsideHeading.querySelector("input"))?.textContent).toBe("Label");
  });

  it("keeps scoped paints stable and ids separate for identical source contexts", () => {
    const source = "- [ ] Same label\n- [x] Same label";
    const body = markdownHtml(source, { taskLabelScope: "task:a/body" });
    const comment = markdownHtml(source, { taskLabelScope: "task:a/comment:c1" });
    const otherComment = markdownHtml(source, { taskLabelScope: "task:a/comment:c2" });
    expect(markdownHtml(source, { taskLabelScope: "task:a/body" })).toBe(body);
    const host = hostOf(body + comment + otherComment);
    const ids = [...host.querySelectorAll("[id]")].map((element) => element.id);
    expect(ids).toHaveLength(6);
    expect(new Set(ids).size).toBe(6);
    expect([...host.querySelectorAll("input")].every((input) => labelOf(input)?.textContent === "Same label")).toBe(true);
    const checked = hostOf(markdownHtml(source.replace("[ ]", "[x]"), { taskLabelScope: "task:a/body" }));
    expect([...checked.querySelectorAll("[id]")].map((element) => element.id)).toEqual(ids.slice(0, 2));
  });

  it("keeps unscoped paints stable and derives fallback names from rendered text", () => {
    const source = '- [ ] **Ship** `release` [notes](https://example.test) "<&"';
    const html = markdownHtml(source);
    expect(markdownHtml(source)).toBe(html);
    const input = hostOf(html).querySelector("input");
    expect(input.getAttribute("aria-label")).toBe('Ship release notes "<&"');
    expect(input.hasAttribute("aria-labelledby")).toBe(false);
    expect(hostOf(html + html).querySelectorAll("[id]")).toHaveLength(0);
  });

  it("uses distinct stable scopes for task bodies, comments, tasks and devices", () => {
    const body = "- [ ] **Release**";
    const context = { deviceId: "d1", projectId: "p1", identities: {} };
    const task = { id: "task-1", body };
    const rows = ["c1", "c2"].map((key) => ({ key, type: "comment", actor: { kind: "user" }, body }));
    const html = taskBodyHtml(task, context) + timelineHtml(rows, context)
      + taskBodyHtml({ id: "task-2", body }, context) + taskBodyHtml(task, { ...context, deviceId: "d2" });
    expect(taskBodyHtml(task, context)).toBe(taskBodyHtml(task, context));
    expect(timelineHtml(rows, context)).toBe(timelineHtml(rows, context));
    const inputs = [...hostOf(html).querySelectorAll('input[type="checkbox"]')];
    expect(inputs).toHaveLength(5);
    const targets = inputs.map((input) => input.getAttribute("aria-labelledby"));
    expect(targets.every(Boolean)).toBe(true);
    expect(new Set(targets).size).toBe(5);
    expect(inputs.every((input) => labelOf(input)?.textContent === "Release")).toBe(true);
  });
});
