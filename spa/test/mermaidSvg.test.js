/** @vitest-environment jsdom */
import { expect, it } from "vitest";
import { sanitizeMermaidSvg } from "../src/core/mermaidSvg.js";

const svg = (body) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 80">${body}</svg>`;
it("rebuilds geometry and text, dropping active elements, CSS, links and external resources", () => {
  const clean = sanitizeMermaidSvg(svg(`<style>@import 'https://evil.test/css'; * { fill:url(https://evil.test/a) }</style>
    <script>alert(1)</script><foreignObject><div xmlns="http://www.w3.org/1999/xhtml">HTML</div></foreignObject>
    <image href="https://evil.test/a"/><use href="https://evil.test/b#c"/>
    <a href="javascript:alert(1)"><text>link</text></a><animate attributeName="href"/>
    <g class="app-shell" style="background:url(https://evil.test/c)" onclick="alert(1)">
      <rect x="1" y="2" width="90" height="50" fill="url(https://evil.test/d)"/>
      <text x="10" y="20">Safe &lt;img src=x&gt;</text></g>`), document, "test");
  expect(clean.querySelector("script, style, foreignObject, image, use, a, animate")).toBeNull();
  expect(clean.querySelector("text").textContent).toBe("Safe <img src=x>");
  expect(clean.outerHTML).not.toMatch(/evil|onclick|class=|style=|href=|url\(/);
  expect(clean.querySelector("rect").getAttribute("width")).toBe("90");
});
it("remaps only unique document-local marker references per insertion", () => {
  const source = svg('<defs><marker id="arrow"><path d="M 0 0 L 5 5 Z"/></marker></defs><path marker-end="url(#arrow)"/><path marker-end="url(https://evil.test/a#arrow)"/>');
  const one = sanitizeMermaidSvg(source, document, "one");
  const two = sanitizeMermaidSvg(source, document, "two");
  expect(one.querySelector("marker").id).not.toBe(two.querySelector("marker").id);
  expect(one.querySelector("svg > path").getAttribute("marker-end")).toBe(`url(#${one.querySelector("marker").id})`);
  expect(one.lastChild.hasAttribute("marker-end")).toBe(false);
  expect(sanitizeMermaidSvg(svg('<marker id="dup"/><marker id="dup"/><path marker-end="url(#dup)"/>'), document, "dup").lastChild.hasAttribute("marker-end")).toBe(false);
});
it("fails closed on malformed, non-SVG and oversized output", () => {
  for (const source of ['<svg>', '<html/>', '<svg xmlns="evil"/>', svg('<g>'.repeat(60) + '</g>'.repeat(60)), svg('x'.repeat(1_000_001))]) {
    expect(() => sanitizeMermaidSvg(source, document, "bad")).toThrow();
  }
});
it("preserves filled versus hollow UML markers without treating marker circles as state dots", () => {
  const clean = sanitizeMermaidSvg(svg('<defs><marker class="marker aggregation"><path d="M 0 0 L 5 5 Z"/></marker><marker class="marker composition"><path d="M 0 0 L 5 5 Z"/></marker><marker class="marker lollipop"><circle r="6"/></marker></defs>'), document, "uml");
  expect(clean.querySelectorAll(".md-svg-marker-hollow")).toHaveLength(2);
  expect(clean.querySelectorAll(".md-svg-marker-filled")).toHaveLength(1);
  expect(clean.querySelector("marker circle").hasAttribute("class")).toBe(false);
});
