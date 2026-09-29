/** @vitest-environment jsdom */
// #229: quotes, list items that hold blocks, and every heading level.
//
// The fixture is the comment that showed the renderer could not do them: the
// agent-instruction wording posted on #229 (tc-01M3PZ8XHR8ZAT55YNN2EC5YDP).
// Its `> ` lines under list items read as a literal ">", its multi-paragraph
// quotes ran together with "> >" inline, and the table inside a quote came out
// as raw pipes.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { markdownHtml } from "../src/core/markdown.js";

const fixture = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "fixtures", "referenceWordingComment.md"), "utf8");

const hostOf = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
};

describe("the #229 wording comment", () => {
  const host = hostOf(markdownHtml(fixture));

  // A block's own words, code spans left out: `<name or id>` is an example.
  it("leaves no quote mark as text outside a code span", () => {
    const marks = [...host.querySelectorAll("p, li, td, th, h2, h3")]
      .map((element) => [...element.childNodes].filter((node) => node.nodeType === 3).map((node) => node.nodeValue).join(""))
      .filter((words) => /(^|\s)>(\s|$)/.test(words));
    expect(host.querySelectorAll("blockquote").length).toBeGreaterThan(0);
    expect(marks).toEqual([]);
  });

  it("puts a quote inside the list item that introduces it", () => {
    const before = [...host.querySelectorAll("li")].find((item) => item.firstChild?.nodeValue?.startsWith("Before:"));
    expect(before?.querySelector(":scope > blockquote")).not.toBeNull();
    expect(before.querySelector("blockquote").textContent).toContain("The full report of this turn");
  });

  it("reads consecutive quote lines, blank ones included, as one quote of several paragraphs", () => {
    const quote = [...host.querySelectorAll("blockquote")]
      .find((one) => one.textContent.startsWith("Write a reference and the reader gets a link. "));
    expect(quote.querySelectorAll(":scope > p")).toHaveLength(3);
    expect(quote.textContent).not.toMatch(/>\s*>/);
  });

  it("renders a table inside a quote", () => {
    const quote = [...host.querySelectorAll("blockquote")].find((one) => one.querySelector("table"));
    expect(quote).toBeDefined();
    expect([...quote.querySelectorAll("th")].map((cell) => cell.textContent)).toEqual(["Write", "Links to"]);
    expect(quote.querySelectorAll("tbody tr")).toHaveLength(8);
    expect(host.textContent).not.toContain("| Write | Links to |");
  });
});

describe("quotes", () => {
  it("wear the renderer's own class", () => {
    expect(markdownHtml("> said")).toBe('<blockquote class="md-quote"><p>said</p></blockquote>');
  });

  it("nest", () => {
    expect(markdownHtml("> outer\n>\n> > inner")).toBe(
      '<blockquote class="md-quote"><p>outer</p><blockquote class="md-quote"><p>inner</p></blockquote></blockquote>',
    );
  });

  it("end a paragraph, and end at the first line without a mark", () => {
    expect(markdownHtml("before\n> quoted\nafter")).toBe(
      '<p>before</p><blockquote class="md-quote"><p>quoted</p></blockquote><p>after</p>',
    );
  });

  it("hold a list and a fence", () => {
    const html = markdownHtml("> - one\n> - two\n>\n> ```\n> <b>code</b>\n> ```");
    expect(html).toContain('<blockquote class="md-quote"><ul><li>one</li><li>two</li></ul>');
    expect(html).toContain("&lt;b&gt;code&lt;/b&gt;");
  });
});

describe("list items that hold blocks", () => {
  it("keeps a one-line item on one line", () => {
    expect(markdownHtml("- one\n- two")).toBe("<ul><li>one</li><li>two</li></ul>");
  });

  it("nests a list indented under an item", () => {
    expect(markdownHtml("- one\n  - inner\n- two")).toBe("<ul><li>one<ul><li>inner</li></ul></li><li>two</li></ul>");
  });

  it("numbers an ordered list", () => {
    expect(markdownHtml("1. first\n2. second")).toBe("<ol><li>first</li><li>second</li></ol>");
  });

  it("keeps items apart by a blank line in one list", () => {
    expect(markdownHtml("- one\n\n- two")).toBe("<ul><li>one</li><li>two</li></ul>");
  });

  it("carries a paragraph indented after a blank line", () => {
    expect(markdownHtml("- one\n\n  more of one\n- two")).toBe("<ul><li>one<p>more of one</p></li><li>two</li></ul>");
  });

  it("ends at a line that is not indented under it", () => {
    expect(markdownHtml("- one\nafter")).toBe("<ul><li>one</li></ul><p>after</p>");
  });
});

describe("headings", () => {
  it("renders every level from one to six", () => {
    for (let level = 1; level <= 6; level += 1) {
      expect(markdownHtml(`${"#".repeat(level)} Level`)).toBe(`<h${level} id="level">Level</h${level}>`);
    }
  });

  it("leaves seven marks as prose", () => {
    expect(markdownHtml("####### Seven")).toBe("<p>####### Seven</p>");
  });
});
