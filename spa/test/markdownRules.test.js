// #253: a thematic break is a divider, not three dashes of text.
//
// CommonMark reads a line of three or more `-`, `*` or `_` — the same
// character throughout, spaces allowed between them, up to three spaces of
// indent — as a break. The #229 wording comment used `---` between its
// sections and it came out as literal text.
//
// A `---` or `===` directly under a line of prose is not a break but that
// line's underline: a setext heading, level 2 and level 1, as CommonMark reads
// it. After a blank line, `---` is a divider.

import { describe, expect, it } from "vitest";

import { markdownHtml } from "../src/core/markdown.js";

const RULE = '<hr class="md-rule">';

describe("a thematic break", () => {
  it("draws a rule for three or more of -, * or _", () => {
    for (const line of ["---", "***", "___", "-----", "*****", "________"]) {
      expect([line, markdownHtml(line)]).toEqual([line, RULE]);
    }
  });

  it("allows spaces between the characters, around them, and up to three spaces of indent", () => {
    for (const line of ["- - -", "* * *", "_ _ _", " -  -  - ", "   ---", "***\t", "-\t-\t-"]) {
      expect([line, markdownHtml(line)]).toEqual([line, RULE]);
    }
  });

  it("is not a rule with fewer than three characters, mixed characters, or anything else on the line", () => {
    const cases = {
      "--": "<p>--</p>",
      "**": "<p>**</p>",
      "-*-": "<p>-*-</p>",
      "---a": "<p>---a</p>",
      "    ---": "<p>---</p>",
    };
    for (const [line, html] of Object.entries(cases)) {
      expect([line, markdownHtml(line)]).toEqual([line, html]);
    }
  });

  it("leaves a list item whose words only start with dashes a list item", () => {
    expect(markdownHtml("- - x")).toBe("<ul><li>- x</li></ul>");
    expect(markdownHtml("- item\n- - x")).toBe("<ul><li>item</li><li>- x</li></ul>");
  });

  it("wins over a list item, as CommonMark reads `- - -` and `* * *`", () => {
    expect(markdownHtml("- one\n- - -\n- two")).toBe(`<ul><li>one</li></ul>${RULE}<ul><li>two</li></ul>`);
    expect(markdownHtml("* * *")).toBe(RULE);
  });

  it("separates the paragraphs around it after a blank line, or directly when it is not an underline", () => {
    expect(markdownHtml("one\n\n---\n\ntwo")).toBe(`<p>one</p>${RULE}<p>two</p>`);
    expect(markdownHtml("one\n***\ntwo")).toBe(`<p>one</p>${RULE}<p>two</p>`);
    expect(markdownHtml("one\n- - -\ntwo")).toBe(`<p>one</p>${RULE}<p>two</p>`);
    expect(markdownHtml("## Section\n---\nbody")).toBe(`<h2 id="section">Section</h2>${RULE}<p>body</p>`);
  });

  it("is drawn inside a quote", () => {
    expect(markdownHtml("> one\n>\n> ---\n> two")).toBe(`<blockquote class="md-quote"><p>one</p>${RULE}<p>two</p></blockquote>`);
    expect(markdownHtml("> one\n> ***\n> two")).toBe(`<blockquote class="md-quote"><p>one</p>${RULE}<p>two</p></blockquote>`);
  });

  it("is drawn inside a list item, on the item's line or under it", () => {
    expect(markdownHtml("- one\n\n  ---\n\n  two")).toBe(`<ul><li>one${RULE}<p>two</p></li></ul>`);
    expect(markdownHtml("* ---")).toBe(`<ul><li>${RULE}</li></ul>`);
  });

  it("is left alone inside a fence and a code span", () => {
    expect(markdownHtml("```\n---\n```")).toBe('<pre class="md-code"><code>---\n</code></pre>');
    expect(markdownHtml("`---`")).toBe("<p><code>---</code></p>");
  });

  it("is still a table's delimiter row under a header row", () => {
    expect(markdownHtml("| a |\n|---|\n| 1 |")).toContain("<table>");
    expect(markdownHtml("a | b\n--- | ---\n1 | 2")).toContain("<table>");
  });

  it("draws nothing in inline mode and takes nothing into a plain preview", () => {
    expect(markdownHtml("one\n---\ntwo", { mode: "inline" })).toBe("one --- two");
    expect(markdownHtml("one\n\n---\n\ntwo", { mode: "plain" })).toBe("one two");
  });
});

describe("a setext heading", () => {
  it("reads prose with `---` directly under it as a level-2 heading", () => {
    expect(markdownHtml("Section\n---\nbody")).toBe('<h2 id="section">Section</h2><p>body</p>');
    expect(markdownHtml("Section\n-")).toBe('<h2 id="section">Section</h2>');
    expect(markdownHtml("Section\n   ------  ")).toBe('<h2 id="section">Section</h2>');
  });

  it("reads prose with `===` directly under it as a level-1 heading", () => {
    expect(markdownHtml("Title\n===\n\nbody")).toBe('<h1 id="title">Title</h1><p>body</p>');
  });

  it("takes every line of the paragraph above as the heading's text", () => {
    expect(markdownHtml("A **long**\ntitle\n=====")).toBe('<h1 id="a-long-title">A <strong>long</strong> title</h1>');
  });

  it("is only an underline after prose: after a blank line `---` is a rule and `===` is text", () => {
    expect(markdownHtml("one\n\n---")).toBe(`<p>one</p>${RULE}`);
    expect(markdownHtml("===")).toBe("<p>===</p>");
    expect(markdownHtml("one\n\n===")).toBe("<p>one</p><p>===</p>");
  });

  it("is read inside a quote and a list item", () => {
    expect(markdownHtml("> Title\n> ===")).toBe('<blockquote class="md-quote"><h1 id="title">Title</h1></blockquote>');
    expect(markdownHtml("- Title\n  ---")).toBe('<ul><li><h2 id="title">Title</h2></li></ul>');
  });

  it("de-duplicates its id with the ATX headings of the same document", () => {
    expect(markdownHtml("## Notes\n\nNotes\n---")).toBe('<h2 id="notes">Notes</h2><h2 id="notes-2">Notes</h2>');
  });
});
