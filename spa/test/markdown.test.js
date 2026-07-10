import { describe, it, expect } from "vitest";
import { renderMarkdown } from "../src/core/markdown.js";
import { esc } from "../src/core/text.js";

describe("esc", () => {
  it("escapes html metacharacters", () => {
    expect(esc('<b a="1">&</b>')).toBe('&lt;b a="1"&gt;&amp;&lt;/b&gt;');
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
    expect(html).toContain("<pre><code>");
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
