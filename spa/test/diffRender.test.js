import { describe, it, expect } from "vitest";
import { diffFilesHtml } from "../src/core/diffRender.js";

const files = [
  {
    path: "src/<x>.rs",
    status: "EDIT",
    add: 1,
    del: 1,
    rows: [
      { t: "hunk", text: "@@ -1,2 +1,2 @@" },
      { t: "del", o: 1, text: "let x = <old>;" },
      { t: "add", n: 1, text: "let x = <new>;" },
      { t: "ctx", o: 2, n: 2, text: "done" },
    ],
  },
];

describe("diffFilesHtml", () => {
  it("renders a file block with the diff stat and rows", () => {
    const html = diffFilesHtml(files);
    expect(html).toContain('class="file"');
    expect(html).toContain('class="fb EDIT"');
    expect(html).toContain('<span class="a">+1</span>');
    expect(html).toContain('<span class="d">−1</span>');
    expect(html).toContain('<tr class="hunk">');
    expect(html).toContain('<tr class="del" data-ln="1">');
    expect(html).toContain('<tr class="add" data-ln="1">');
    expect(html).toContain('<tr class="ctx" data-ln="2">');
  });

  it("escapes untrusted file paths and code content", () => {
    const html = diffFilesHtml(files);
    expect(html).not.toContain("src/<x>.rs");
    expect(html).not.toContain("let x = <old>;");
    expect(html).toContain("src/&lt;x&gt;.rs");
    expect(html).toContain("let x = &lt;old&gt;;");
    expect(html).toContain("let x = &lt;new&gt;;");
  });

  it("uses data-file on the file block for comment anchoring", () => {
    expect(diffFilesHtml(files)).toContain('data-file="src/&lt;x&gt;.rs"');
  });
});
