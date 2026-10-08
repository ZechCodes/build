// @vitest-environment jsdom
import { existsSync, readFileSync } from "node:fs";
import { URL as NodeURL } from "node:url";
import { beforeEach, expect, it } from "vitest";
import { toolbarHtml } from "../src/core/toolbarRender.js";
import { tabShellHtml } from "../src/core/tabshell.js";

const interactionSheet = new NodeURL("../src/styles/interactions.css", import.meta.url);
const styles = () => existsSync(interactionSheet) ? readFileSync(interactionSheet, "utf8") : "";

beforeEach(() => {
  document.head.innerHTML = `<style>${styles()}</style>`;
  document.body.innerHTML = `${toolbarHtml({ project: "Build", kind: "task", label: "#397 Interaction audit" })}
    ${tabShellHtml({ tabs: [{ id: "task", label: "Task" }], active: "task" })}
    <div class="task-page-body markdown"><p>Copy this task description.</p><label for="check"><input id="check" type="checkbox">Include archived</label></div>
    <div class="task-comment-body markdown"><p>Copy this comment.</p></div>
    <h1 class="task-page-title">Copy this task title.</h1>
    <input id="name" value="A task name"><textarea id="comment">A comment draft</textarea>
    <select><option>In progress</option></select><button disabled>Comment</button>`;
});

it("imports one shared interaction policy from the production CSS entry", () => {
  expect(readFileSync(new NodeURL("../src/styles.css", import.meta.url), "utf8")).toMatch(/@import\s+["']\.\/styles\/interactions\.css["']/);
  expect(getComputedStyle(document.body).userSelect).toBe("none");
});

it("restores selection for documents and native fields while controls inside prose remain chrome", () => {
  for (const selector of [".task-page-body", ".task-comment-body", ".task-page-title", "input", "textarea", "select"]) {
    expect(getComputedStyle(document.querySelector(selector)).userSelect, selector).toBe("text");
  }
  for (const selector of ["label", "button"]) expect(getComputedStyle(document.querySelector(selector)).userSelect, selector).toBe("none");
  expect(getComputedStyle(document.querySelector("button:disabled")).cursor).toBe("default");
});

it("sets touch callouts explicitly for content and controls without suppressing terminal gestures", () => {
  // Chromium cannot expose this WebKit-only property's computed value. Keep
  // its declaration in the production policy; real gestures run in Chromium.
  expect(styles()).toMatch(/-webkit-touch-callout\s*:\s*none/);
  expect(styles()).toMatch(/-webkit-touch-callout\s*:\s*default/);
  expect(styles()).not.toMatch(/(?:^|[;{])\s*touch-action\s*:\s*none/);
});
