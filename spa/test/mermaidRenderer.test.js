/** @vitest-environment jsdom */
import { beforeEach, expect, it, vi } from "vitest";
const library = vi.hoisted(() => ({ initialize: vi.fn(), render: vi.fn() }));
vi.mock("mermaid", () => ({ default: library }));
let renderer;
const output = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><text x="1" y="2">Safe</text></svg>';
beforeEach(async () => {
  vi.resetModules();
  library.initialize.mockReset();
  library.render.mockReset().mockResolvedValue({ svg: output });
  renderer = await import("../src/core/mermaidRenderer.js");
});
it("rejects dangerous syntax before loading or rendering measurement DOM", async () => {
  const hostile = [
    '%%{init: {"securityLevel":"loose"}}%%\nflowchart LR\nA-->B',
    '---\nconfig:\n securityLevel: loose\n---\nflowchart LR\nA-->B',
    'flowchart LR\nA[<img src="https://evil.test/x">]',
    'flowchart LR\nA[&lt;img src=x&gt;]',
    'flowchart LR\nA[\\u003cimg]',
    'flowchart LR\nA@{ img: "https://evil.test/x" }',
    'flowchart LR\nA-->B; classDef default fill:url(https://evil.test/x)',
    'flowchart LR\nA-->B; style A fill:red',
    'flowchart LR\nA-->B; click A "javascript:alert(1)"',
    'flowchart LR\nA["`HTML label`"]',
    'sequenceDiagram\nparticipant Alice\nparticipant Bob\nproperties Alice: {"icon":"//evil.test/pixel"}\nAlice->>Bob: Hello',
    'sequenceDiagram\nparticipant Alice\nparticipant Bob\ndetails Alice: {"properties":{"icon":"/relative/pixel"}}\nAlice->>Bob: Hello',
    'x'.repeat(16_385),
  ];
  for (const source of hostile) await expect(renderer.renderMermaid(source, "light")).rejects.toThrow();
  expect(library.initialize).not.toHaveBeenCalled();
  expect(library.render).not.toHaveBeenCalled();
  expect(document.querySelector(".md-mermaid-measure")).toBeNull();
});
it("pins strict text-only configuration, caches source/theme and gives copies unique IDs", async () => {
  const source = "flowchart LR\nA-->B";
  library.render.mockResolvedValue({ svg: output.replace('<text', '<marker id="arrow"/><path marker-end="url(#arrow)"/><text') });
  const one = await renderer.renderMermaid(source, "light");
  const two = await renderer.renderMermaid(source, "light");
  expect(library.render).toHaveBeenCalledTimes(1);
  expect(one.querySelector("marker").id).not.toBe(two.querySelector("marker").id);
  await renderer.renderMermaid(source, "dark");
  expect(library.render).toHaveBeenCalledTimes(2);
  expect(library.initialize).toHaveBeenLastCalledWith(expect.objectContaining({ securityLevel: "strict", htmlLabels: false, startOnLoad: false, suppressErrorRendering: true, theme: "dark", flowchart: { htmlLabels: false } }));
  expect(document.querySelector(".md-mermaid-measure")).toBeNull();
});
it("serializes library globals across themes and recovers after parser errors", async () => {
  let rejectFirst;
  library.render.mockImplementationOnce(() => new Promise((_, reject) => { rejectFirst = reject; }));
  const first = renderer.renderMermaid("bad first", "light");
  const firstError = expect(first).rejects.toThrow("parse failed");
  await vi.waitFor(() => expect(library.render).toHaveBeenCalledTimes(1));
  const second = renderer.renderMermaid("flowchart LR\nB-->C", "dark");
  await Promise.resolve();
  expect(library.initialize).toHaveBeenCalledTimes(1);
  rejectFirst(new Error("parse failed"));
  await firstError;
  expect(await second).toBeInstanceOf(SVGElement);
  expect(library.initialize).toHaveBeenCalledTimes(2);
  expect(document.querySelector(".md-mermaid-measure")).toBeNull();
});
it("evicts old cached renders at the 32-entry bound", async () => {
  for (let index = 0; index < 33; index++) await renderer.renderMermaid(`flowchart LR\nA${index}-->B`, "light");
  await renderer.renderMermaid("flowchart LR\nA0-->B", "light");
  expect(library.render).toHaveBeenCalledTimes(34);
});
