/** @vitest-environment jsdom */
import { expect, it } from "vitest";
import { markdownHtml } from "../src/core/markdown.js";

it("marks Mermaid fences for lazy enhancement while retaining escaped source", () => {
  const source = 'flowchart LR\n  A["<script>alert(1)</script>"] --> B';
  document.body.innerHTML = markdownHtml(`\`\`\`mermaid\n${source}\n\`\`\``);
  const block = document.querySelector(".md-mermaid");
  expect(block).not.toBeNull();
  expect(block.querySelector("code").textContent).toBe(`${source}\n`);
  expect(block.querySelector("script, svg")).toBeNull();
});

it("leaves other fences and inline code unchanged", () => {
  expect(markdownHtml("```javascript\nconst x = 1;\n``` ")).toContain('class="language-javascript"');
  expect(markdownHtml("```mermaidx\nA --> B\n```")).not.toContain("md-mermaid");
  expect(markdownHtml("`mermaid`", { mode: "inline" })).toBe("<code>mermaid</code>");
});

it("keeps incomplete streamed fences as ordinary source", () => {
  expect(markdownHtml("```mermaid\nflowchart LR\nA-->B")).not.toContain("md-mermaid");
});
