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
  it("renders headings, paragraphs and inline formatting", () => {
    const html = renderMarkdown("# Title\n## Sub\n### Deep\nSome **bold** and `code`.");
    expect(html).toContain("<h1>Title</h1>");
    expect(html).toContain("<h2>Sub</h2>");
    expect(html).toContain("<h3>Deep</h3>");
    expect(html).toContain("<strong>bold</strong>");
    expect(html).toContain("<code>code</code>");
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
