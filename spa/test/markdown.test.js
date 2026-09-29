import { afterEach, beforeEach, describe, it, expect } from "vitest";
import { markdownHtml } from "../src/core/markdown.js";
import { holdReferenceSources } from "../src/core/referenceIndex.js";
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
    const html = markdownHtml("# Title\n## Sub\n### Deep\nSome **bold** and `code`.");
    expect(html).toContain('<h1 id="title">Title</h1>');
    expect(html).toContain('<h2 id="sub">Sub</h2>');
    expect(html).toContain('<h3 id="deep">Deep</h3>');
    expect(html).toContain("<strong>bold</strong>");
    expect(html).toContain("<code>code</code>");
  });

  it("derives the heading id from the raw text, stripping inline markers", () => {
    const html = markdownHtml("## The `users` table");
    expect(html).toContain('<h2 id="the-users-table">');
    expect(html).toContain("<code>users</code>");
  });

  it("disambiguates duplicate headings with numeric suffixes in document order", () => {
    const html = markdownHtml("## Tables\n## Tables\n## Tables");
    expect(html).toContain('<h2 id="tables">');
    expect(html).toContain('<h2 id="tables-2">');
    expect(html).toContain('<h2 id="tables-3">');
  });

  it("omits the id attribute for a heading with no slug characters", () => {
    const html = markdownHtml("## !!!");
    expect(html).toContain("<h2>");
    expect(html).not.toContain('id=""');
  });

  it("renders bullet and numbered lists", () => {
    const html = markdownHtml("- one\n- two\n\n1. first\n2. second");
    expect(html).toContain("<li>one</li>");
    expect(html).toContain("<li>second</li>");
  });

  it("renders fenced code blocks verbatim (escaped)", () => {
    const html = markdownHtml("```\n<script>alert(1)</script>\n```");
    expect(html).toContain('<pre class="md-code"><code>');
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<script>");
  });

  it("closes unterminated lists and code fences", () => {
    expect(markdownHtml("- dangling")).toContain("</ul>");
    expect(markdownHtml("```\nunclosed")).toContain("</code></pre>");
  });

  it("escapes html in regular text", () => {
    expect(markdownHtml('<img src=x onerror="x">')).not.toContain("<img");
  });

  it("handles empty input", () => {
    expect(markdownHtml("")).toBe("");
    expect(markdownHtml(null)).toBe("");
  });
});

// Pipe tables are how an agent reports a run: a row per case, a column per
// number. Rendered as text they were the least readable thing in the thread —
// six lines of pipes for what a table says at a glance.
describe("renderMarkdown pipe tables", () => {
  const TABLE = ["| Run | Precision | Violations |", "|---|---|---|", "| mention-qa | 100% (2/2) | 0 |", "| reply-chain-join | 100% (3/3) | 0 |"].join("\n");

  it("renders a header row, a body row per line, and nothing left as text", () => {
    const html = markdownHtml(TABLE);
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
    expect(markdownHtml(TABLE)).toContain('<div class="mdtable">');
  });

  it("takes each column's alignment from the delimiter row", () => {
    const html = markdownHtml("| a | b | c | d |\n|:---|---:|:---:|---|\n| 1 | 2 | 3 | 4 |");
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
    const html = markdownHtml("a | b\n--- | ---\n1 | 2");
    expect(html).toContain("<th>a</th>");
    expect(html).toContain("<td>2</td>");
  });

  it("renders inline markdown inside cells, and escapes their html", () => {
    const html = markdownHtml("| what | how |\n|---|---|\n| **bold** | `code` |\n| <img src=x> | plain |");
    expect(html).toContain("<td><strong>bold</strong></td>");
    expect(html).toContain("<td><code>code</code></td>");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img src=x&gt;");
  });

  it("keeps an escaped pipe inside its cell", () => {
    const html = markdownHtml("| pattern | note |\n|---|---|\n| a \\| b | alternation |");
    expect(html).toContain("<td>a | b</td>");
    expect(html).toContain("<td>alternation</td>");
  });

  it("pads a short row and drops what overflows the header", () => {
    const html = markdownHtml("| a | b | c |\n|---|---|---|\n| 1 |\n| 1 | 2 | 3 | 4 |");
    expect(html).toContain("<tr><td>1</td><td></td><td></td></tr>");
    expect(html).toContain("<tr><td>1</td><td>2</td><td>3</td></tr>");
    expect(html).not.toContain("<td>4</td>");
  });

  it("ends the table at the first line that is not a row", () => {
    const html = markdownHtml("| a |\n|---|\n| 1 |\n\nAfter the table.");
    expect(html).toContain("</table>");
    expect(html.indexOf("</table>")).toBeLessThan(html.indexOf("<p>After the table.</p>"));
  });

  it("leaves a pipe line with no delimiter row as a paragraph", () => {
    const html = markdownHtml("| not | a table |\nplain text");
    expect(html).not.toContain("<table>");
    expect(html).toBe("<p>| not | a table | plain text</p>");
  });

  it("leaves pipes inside a code fence alone", () => {
    const html = markdownHtml("```\n| a | b |\n|---|---|\n```");
    expect(html).not.toContain("<table>");
    expect(html).toContain("| a | b |");
  });

  it("closes a table the document ended in the middle of", () => {
    const html = markdownHtml("| a |\n|---|\n| 1 |");
    expect(html).toContain("</tbody></table>");
    expect(html.split("<table>")).toHaveLength(2);
  });

  it("renders a header-only table", () => {
    const html = markdownHtml("| a | b |\n|---|---|");
    expect(html).toContain("<th>a</th>");
    expect(html).not.toContain("<tbody>");
  });

  it("interrupts a list, so a table after bullets is still a table", () => {
    const html = markdownHtml("- one\n\n| a |\n|---|\n| 1 |");
    expect(html).toContain("</ul>");
    expect(html).toContain("<table>");
    expect(html.indexOf("</ul>")).toBeLessThan(html.indexOf("<table>"));
  });
});

// #56. The references an agent can write, through the renderer itself — and
// since #229, resolved against the one index every surface shares.
describe("references an agent writes", () => {
  const place = { deviceId: "d1", projectId: "p1" };
  beforeEach(() => holdReferenceSources({
    feed: {
      projects: [{ id: "p1", deviceId: "d1", projectKey: "d1/p1", name: "Build" }],
      workspaces: [{ id: "ws-1", workspace_id: "ws-1", name: "board", projectKey: "d1/p1" }],
      items: [],
    },
    tasks: { "d1/p1": [{ id: "i-42", number: 42, title: "Rebuild" }] },
  }));
  afterEach(() => holdReferenceSources({}));

  it("links them in a paragraph, a list and a heading", () => {
    expect(markdownHtml("see #42", { place })).toContain('<a class="md-ref" href=');
    expect(markdownHtml("- see #42", { place })).toMatch(/<li>see <a class="md-ref" href=/);
    expect(markdownHtml("## About #42", { place })).toMatch(/<h2[^>]*>About <a class="md-ref" href=/);
  });

  // A heading is a LINE rule and needs the space; `#42` alone is a reference,
  // and neither reading is allowed to eat the other.
  it("keeps a heading a heading and a task a task", () => {
    expect(markdownHtml("# A heading", { place })).toMatch(/<h1[^>]*>A heading<\/h1>/);
    expect(markdownHtml("#42", { place })).toMatch(/^<p><a class="md-ref" href=/);
  });

  it("leaves a fenced block entirely alone", () => {
    const html = markdownHtml("```\nsee #42\n```", { place });
    expect(html).toContain("see #42");
    expect(html).not.toContain("<a ");
  });

  it("leaves a code span alone while linking beside it", () => {
    const html = markdownHtml("write `#42` to reach #42", { place });
    expect(html).toContain("<code>#42</code>");
    expect(html.match(/<a /g)).toHaveLength(1);
  });

  it("keeps URL fragments literal while linking task references beside them", () => {
    const html = markdownHtml("https://example.test/#42/c/tc-7 and https://example.test/?a=1&b=2#42. See #42/c/tc-7", { place });
    expect(html).toContain("https://example.test/#42/c/tc-7");
    expect(html).toContain("https://example.test/?a=1&amp;b=2#42");
    expect(html.match(/<a /g)).toHaveLength(1);
    expect(html).toContain(">#42 Rebuild · comment</a>");
  });

  // Without a project to stand in, `#42` names nothing anyone can find — but
  // a workspace is named across the whole account, so it still links.
  it("resolves what it can without a place", () => {
    expect(markdownHtml("see #42")).toBe("<p>see #42</p>");
    expect(markdownHtml("on @workspace:board")).toContain('<a class="md-ref"');
  });

  it("marks a reference the index looked for and did not find", () => {
    expect(markdownHtml("see #999", { place })).toMatch(/^<p>see <span class="md-ref-missing" title="[^"]+">#999<\/span><\/p>$/);
  });

  it("renders every form as plain words before the index has been read", () => {
    holdReferenceSources({});
    expect(markdownHtml("see #42 on @workspace:board", { place })).toBe("<p>see #42 on @workspace:board</p>");
  });
});

// #229: one entry point, three shapes — the same references and the same
// escaping in each.
describe("the modes of the one renderer", () => {
  const place = { deviceId: "d1", projectId: "p1" };
  beforeEach(() => holdReferenceSources({
    feed: { projects: [], workspaces: [{ id: "ws-1", workspace_id: "ws-1", name: "board", projectKey: "d1/p1" }], items: [] },
    tasks: { "d1/p1": [{ id: "i-42", number: 42, title: "Rebuild" }] },
  }));
  afterEach(() => holdReferenceSources({}));

  it("renders one line inline: no paragraph, no blocks, lines joined", () => {
    expect(markdownHtml("**Done** for\n#42 <b>", { place, mode: "inline" }))
      .toMatch(/^<strong>Done<\/strong> for <a class="md-ref"[^>]*>#42 Rebuild<\/a> &lt;b&gt;$/);
  });

  it("renders plain text with each reference read as its words", () => {
    expect(markdownHtml("## Shipped\n\n**Done** on [[board:commit:b8ce4ee9]] for #42", { place, mode: "plain" }))
      .toBe("Shipped Done on board · b8ce4ee9 for #42 Rebuild");
  });

  it("cuts plain text at the limit it is given", () => {
    expect(markdownHtml("a ".repeat(50), { mode: "plain", limit: 10 })).toBe("a a a a a…");
  });
});

describe("renderMarkdown paragraphs", () => {
  it("joins wrapped lines into one paragraph with a space", () => {
    expect(markdownHtml("one line\n  wrapped here  \nand here")).toBe("<p>one line wrapped here<br>and here</p>");
    expect(markdownHtml("one line\nwrapped here")).toBe("<p>one line wrapped here</p>");
  });

  it("splits paragraphs on a blank line", () => {
    expect(markdownHtml("first\n\nsecond\n  \nthird")).toBe("<p>first</p><p>second</p><p>third</p>");
  });

  it("breaks the line on two trailing spaces or a backslash", () => {
    expect(markdownHtml("roses  \nviolets\\\nsugar")).toBe("<p>roses<br>violets<br>sugar</p>");
  });

  it("closes the paragraph before a heading", () => {
    expect(markdownHtml("text\n# Heading\nmore")).toBe('<p>text</p><h1 id="heading">Heading</h1><p>more</p>');
  });

  it("closes the paragraph before a list item", () => {
    expect(markdownHtml("text\n- item")).toBe("<p>text</p><ul><li>item</li></ul>");
  });

  it("closes the paragraph before a table", () => {
    expect(markdownHtml("text\n| a |\n| - |\n| 1 |")).toBe(
      '<p>text</p><div class="mdtable"><table><thead><tr><th>a</th></tr></thead><tbody><tr><td>1</td></tr></tbody></table></div>',
    );
  });

  it("closes the paragraph around a code fence", () => {
    expect(markdownHtml("before\n```\ncode  \n```\nafter")).toBe(
      '<p>before</p><pre class="md-code"><code>code  \n</code></pre><p>after</p>',
    );
  });
});
