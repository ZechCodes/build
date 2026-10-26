/** @vitest-environment jsdom */
import { afterEach, expect, it, vi } from "vitest";
import { markdownHtml } from "../src/core/markdown.js";
import { installMarkdownMermaid } from "../src/core/markdownMermaid.js";
const source = (text) => markdownHtml(`\`\`\`mermaid\n${text}\n\`\`\``);
const diagram = (text) => {
  const node = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  node.textContent = text;
  return node;
};
let dispose = () => {};
afterEach(() => { dispose(); document.body.replaceChildren(); delete document.documentElement.dataset.theme; });

it("loads only for Mermaid blocks and enhances later cached DOM paints without connection state", async () => {
  const renderMermaid = vi.fn(async () => diagram("ready"));
  const load = vi.fn(async () => ({ renderMermaid }));
  dispose = installMarkdownMermaid(document, { load });
  document.body.innerHTML = markdownHtml("```js\nconst n = 1;\n```");
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(load).not.toHaveBeenCalled();
  document.body.innerHTML = source("flowchart LR\nA-->B");
  await vi.waitFor(() => expect(document.querySelector(".md-mermaid > svg")?.textContent).toBe("ready"));
  expect(document.querySelector("pre").hidden).toBe(true);
  expect(renderMermaid).toHaveBeenCalledTimes(1);
});
it("retains code and a quiet fixed error when rendering fails", async () => {
  dispose = installMarkdownMermaid(document, { load: async () => ({ renderMermaid: async () => { throw new Error('<img src=x>'); } }) });
  document.body.innerHTML = source("not a diagram");
  await vi.waitFor(() => expect(document.querySelector(".md-mermaid-error")?.textContent).toBe("Diagram unavailable"));
  expect(document.querySelector("code").textContent).toBe("not a diagram\n");
  expect(document.querySelector("pre").hidden).toBe(false);
  expect(document.querySelector("svg, img")).toBeNull();
});
it("fences stale results after a theme or source change and on teardown", async () => {
  const pending = [];
  const renderMermaid = vi.fn((text, theme) => new Promise((resolve) => pending.push(() => resolve(diagram(`${theme}:${text}`)))));
  document.body.innerHTML = source("flowchart LR\nA-->B");
  dispose = installMarkdownMermaid(document, { load: async () => ({ renderMermaid }) });
  await vi.waitFor(() => expect(pending).toHaveLength(1));
  document.documentElement.dataset.theme = "dark";
  await vi.waitFor(() => expect(pending).toHaveLength(2));
  pending[0]();
  await Promise.resolve();
  expect(document.querySelector("svg")).toBeNull();
  pending[1]();
  await vi.waitFor(() => expect(document.querySelector("svg")?.textContent).toContain("dark:"));
  document.querySelector("code").textContent = "flowchart LR\nB-->C";
  await vi.waitFor(() => expect(pending).toHaveLength(3));
  dispose();
  pending[2]();
  await Promise.resolve();
  expect(document.querySelector("svg").textContent).toContain("A-->B");
});
it("does not paint detached blocks and can enhance them when reinserted", async () => {
  let finish;
  const renderMermaid = vi.fn(() => new Promise((resolve) => { finish = resolve; }));
  document.body.innerHTML = source("flowchart LR\nA-->B");
  const block = document.body.firstChild;
  dispose = installMarkdownMermaid(document, { load: async () => ({ renderMermaid }) });
  await vi.waitFor(() => expect(renderMermaid).toHaveBeenCalledTimes(1));
  block.remove(); finish(diagram("detached"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(block.querySelector("svg")).toBeNull();
  document.body.append(block);
  await vi.waitFor(() => expect(renderMermaid).toHaveBeenCalledTimes(2));
  finish(diagram("reinserted"));
  await vi.waitFor(() => expect(block.querySelector("svg")?.textContent).toBe("reinserted"));
});
