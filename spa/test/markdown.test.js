import { describe, it, expect } from "vitest";
import { renderMarkdown } from "../src/core/markdown.js";
import { esc } from "../src/core/text.js";

describe("esc", () => {
  it("escapes html metacharacters (quotes included — attribute contexts)", () => {
    expect(esc('<b a="1">&</b>')).toBe("&lt;b a=&quot;1&quot;&gt;&amp;&lt;/b&gt;");
  });
  it("tolerates null/undefined", () => {
    expect(esc(null)).toBe("");
    expect(esc(undefined)).toBe("");
  });
});

describe("renderMarkdown", () => {
  it("renders headings with slug ids, paragraphs and inline formatting", () => {
    const html = renderMarkdown("# Title\n## Sub\n### Deep\nSome **bold** and `code`.");
    expect(html).toContain('<h1 id="title">Title</h1>');
    expect(html).toContain('<h2 id="sub">Sub</h2>');
    expect(html).toContain('<h3 id="deep">Deep</h3>');
    expect(html).toContain("<strong>bold</strong>");
    expect(html).toContain("<code>code</code>");
  });

  it("derives the heading id from the raw text, stripping inline markers", () => {
    const html = renderMarkdown("## The `users` table");
    expect(html).toContain('<h2 id="the-users-table">');
    expect(html).toContain("<code>users</code>");
  });

  it("disambiguates duplicate headings with numeric suffixes in document order", () => {
    const html = renderMarkdown("## Tables\n## Tables\n## Tables");
    expect(html).toContain('<h2 id="tables">');
    expect(html).toContain('<h2 id="tables-2">');
    expect(html).toContain('<h2 id="tables-3">');
  });

  it("omits the id attribute for a heading with no slug characters", () => {
    const html = renderMarkdown("## !!!");
    expect(html).toContain("<h2>");
    expect(html).not.toContain('id=""');
  });

  it("renders bullet and numbered lists", () => {
    const html = renderMarkdown("- one\n- two\n\n1. first\n2. second");
    expect(html).toContain("<li>one</li>");
    expect(html).toContain("<li>second</li>");
  });

  it("renders fenced code blocks verbatim (escaped)", () => {
    const html = renderMarkdown("```\n<script>alert(1)</script>\n```");
    expect(html).toContain('<pre class="md-code"><code>');
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<script>");
  });

  it("closes unterminated lists and code fences", () => {
    expect(renderMarkdown("- dangling")).toContain("</ul>");
    expect(renderMarkdown("```\nunclosed")).toContain("</code></pre>");
  });

  it("escapes html in regular text", () => {
    expect(renderMarkdown('<img src=x onerror="x">')).not.toContain("<img");
  });

  it("handles empty input", () => {
    expect(renderMarkdown("")).toBe("");
    expect(renderMarkdown(null)).toBe("");
  });
});

// Pipe tables are how an agent reports a run: a row per case, a column per
// number. Rendered as text they were the least readable thing in the thread —
// six lines of pipes for what a table says at a glance.
describe("renderMarkdown pipe tables", () => {
  const TABLE = ["| Run | Precision | Violations |", "|---|---|---|", "| mention-qa | 100% (2/2) | 0 |", "| reply-chain-join | 100% (3/3) | 0 |"].join("\n");

  it("renders a header row, a body row per line, and nothing left as text", () => {
    const html = renderMarkdown(TABLE);
    expect(html).toContain("<table>");
    expect(html).toContain("<thead><tr><th>Run</th><th>Precision</th><th>Violations</th></tr></thead>");
    expect(html).toContain("<td>mention-qa</td>");
    expect(html).toContain("<td>reply-chain-join</td>");
    expect(html).not.toContain("|---|");
    expect(html).not.toContain("<p>| Run");
  });

  it("scrolls a wide table inside its own box rather than widening the message", () => {
    // The thread panel is a narrow column; a table wider than it has to scroll
    // in place, or it takes the conversation's width with it.
    expect(renderMarkdown(TABLE)).toContain('<div class="mdtable">');
  });

  it("takes each column's alignment from the delimiter row", () => {
    const html = renderMarkdown("| a | b | c | d |\n|:---|---:|:---:|---|\n| 1 | 2 | 3 | 4 |");
    expect(html).toContain('<th style="text-align:left">a</th>');
    expect(html).toContain('<th style="text-align:right">b</th>');
    expect(html).toContain('<th style="text-align:center">c</th>');
    expect(html).toContain("<th>d</th>");
    expect(html).toContain('<td style="text-align:left">1</td>');
    expect(html).toContain('<td style="text-align:right">2</td>');
    expect(html).toContain('<td style="text-align:center">3</td>');
    expect(html).toContain("<td>4</td>");
  });

  it("reads rows with or without the outer pipes", () => {
    const html = renderMarkdown("a | b\n--- | ---\n1 | 2");
    expect(html).toContain("<th>a</th>");
    expect(html).toContain("<td>2</td>");
  });

  it("renders inline markdown inside cells, and escapes their html", () => {
    const html = renderMarkdown("| what | how |\n|---|---|\n| **bold** | `code` |\n| <img src=x> | plain |");
    expect(html).toContain("<td><strong>bold</strong></td>");
    expect(html).toContain("<td><code>code</code></td>");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img src=x&gt;");
  });

  it("keeps an escaped pipe inside its cell", () => {
    const html = renderMarkdown("| pattern | note |\n|---|---|\n| a \\| b | alternation |");
    expect(html).toContain("<td>a | b</td>");
    expect(html).toContain("<td>alternation</td>");
  });

  it("pads a short row and drops what overflows the header", () => {
    const html = renderMarkdown("| a | b | c |\n|---|---|---|\n| 1 |\n| 1 | 2 | 3 | 4 |");
    expect(html).toContain("<tr><td>1</td><td></td><td></td></tr>");
    expect(html).toContain("<tr><td>1</td><td>2</td><td>3</td></tr>");
    expect(html).not.toContain("<td>4</td>");
  });

  it("ends the table at the first line that is not a row", () => {
    const html = renderMarkdown("| a |\n|---|\n| 1 |\n\nAfter the table.");
    expect(html).toContain("</table>");
    expect(html.indexOf("</table>")).toBeLessThan(html.indexOf("<p>After the table.</p>"));
  });

  it("leaves a pipe line with no delimiter row as a paragraph", () => {
    const html = renderMarkdown("| not | a table |\nplain text");
    expect(html).not.toContain("<table>");
    expect(html).toBe("<p>| not | a table | plain text</p>");
  });

  it("leaves pipes inside a code fence alone", () => {
    const html = renderMarkdown("```\n| a | b |\n|---|---|\n```");
    expect(html).not.toContain("<table>");
    expect(html).toContain("| a | b |");
  });

  it("closes a table the document ended in the middle of", () => {
    const html = renderMarkdown("| a |\n|---|\n| 1 |");
    expect(html).toContain("</tbody></table>");
    expect(html.split("<table>")).toHaveLength(2);
  });

  it("renders a header-only table", () => {
    const html = renderMarkdown("| a | b |\n|---|---|");
    expect(html).toContain("<th>a</th>");
    expect(html).not.toContain("<tbody>");
  });

  it("interrupts a list, so a table after bullets is still a table", () => {
    const html = renderMarkdown("- one\n\n| a |\n|---|\n| 1 |");
    expect(html).toContain("</ul>");
    expect(html).toContain("<table>");
    expect(html.indexOf("</ul>")).toBeLessThan(html.indexOf("<table>"));
  });
});

// #56. The references an agent can write, through the renderer itself.
describe("references an agent writes", () => {
  const links = {
    issue: (number) => (number === 42 ? { deviceId: "d1", projectId: "p1", issueId: "i-42", title: "Rebuild" } : null),
    workspace: (name) => (name === "board" ? { deviceId: "d1", projectId: "p1", workspaceId: "ws-1" } : null),
  };

  it("links them in a paragraph, a list and a heading", () => {
    expect(renderMarkdown("see #42", { links })).toContain("<a href=");
    expect(renderMarkdown("- see #42", { links })).toMatch(/<li>see <a href=/);
    expect(renderMarkdown("## About #42", { links })).toMatch(/<h2[^>]*>About <a href=/);
  });

  // A heading is a LINE rule and needs the space; `#42` alone is a reference,
  // and neither reading is allowed to eat the other.
  it("keeps a heading a heading and an issue an issue", () => {
    expect(renderMarkdown("# A heading", { links })).toMatch(/<h1[^>]*>A heading<\/h1>/);
    expect(renderMarkdown("#42", { links })).toMatch(/^<p><a href=/);
  });

  it("leaves a fenced block entirely alone", () => {
    const html = renderMarkdown("```\nsee #42\n```", { links });
    expect(html).toContain("see #42");
    expect(html).not.toContain("<a href=");
  });

  it("leaves a code span alone while linking beside it", () => {
    const html = renderMarkdown("write `#42` to reach #42", { links });
    expect(html).toContain("<code>#42</code>");
    expect(html.match(/<a /g)).toHaveLength(1);
  });

  it("keeps URL fragments literal while linking issue references beside them", () => {
    const html = renderMarkdown("https://example.test/#42/c/ic-7 and https://example.test/?a=1&b=2#42. See #42/c/ic-7", { links });
    expect(html).toContain("https://example.test/#42/c/ic-7");
    expect(html).toContain("https://example.test/?a=1&amp;b=2#42");
    expect(html.match(/<a /g)).toHaveLength(1);
    expect(html).toContain(">#42/c/ic-7</a>");
  });

  it("renders as plain words with no resolver, which is every caller today", () => {
    expect(renderMarkdown("see #42")).toBe("<p>see #42</p>");
  });

  it("renders an unknown target as plain words", () => {
    expect(renderMarkdown("see #999", { links })).toBe("<p>see #999</p>");
  });
});

describe("renderMarkdown paragraphs", () => {
  it("joins wrapped lines into one paragraph with a space", () => {
    expect(renderMarkdown("one line\n  wrapped here  \nand here")).toBe("<p>one line wrapped here<br>and here</p>");
    expect(renderMarkdown("one line\nwrapped here")).toBe("<p>one line wrapped here</p>");
  });

  it("splits paragraphs on a blank line", () => {
    expect(renderMarkdown("first\n\nsecond\n  \nthird")).toBe("<p>first</p><p>second</p><p>third</p>");
  });

  it("breaks the line on two trailing spaces or a backslash", () => {
    expect(renderMarkdown("roses  \nviolets\\\nsugar")).toBe("<p>roses<br>violets<br>sugar</p>");
  });

  it("closes the paragraph before a heading", () => {
    expect(renderMarkdown("text\n# Heading\nmore")).toBe('<p>text</p><h1 id="heading">Heading</h1><p>more</p>');
  });

  it("closes the paragraph before a list item", () => {
    expect(renderMarkdown("text\n- item")).toBe("<p>text</p><ul><li>item</li></ul>");
  });

  it("closes the paragraph before a table", () => {
    expect(renderMarkdown("text\n| a |\n| - |\n| 1 |")).toBe(
      '<p>text</p><div class="mdtable"><table><thead><tr><th>a</th></tr></thead><tbody><tr><td>1</td></tr></tbody></table></div>',
    );
  });

  it("closes the paragraph around a code fence", () => {
    expect(renderMarkdown("before\n```\ncode  \n```\nafter")).toBe(
      '<p>before</p><pre class="md-code"><code>code  \n</code></pre><p>after</p>',
    );
  });
});
