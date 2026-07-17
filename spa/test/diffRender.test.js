import { describe, it, expect } from "vitest";
import { diffFilesHtml, diffRowsHtml } from "../src/core/diffRender.js";

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
    expect(html).toContain('class="file capped"');
    expect(html).toContain('class="fb EDIT"');
    expect(html).toContain('<span class="a">+1</span>');
    expect(html).toContain('<span class="d">−1</span>');
    expect(html).toContain('<tr class="hunk">');
    expect(html).toContain('<tr class="del" data-ln="1">');
    expect(html).toContain('<tr class="add" data-ln="1">');
    expect(html).toContain('<tr class="ctx" data-ln="2">');
  });

  it("wraps the table in a .dscroll box so the code scrolls under a fixed header", () => {
    const html = diffFilesHtml(files);
    expect(html).toContain('<div class="dscroll"><table>');
    expect(html).toContain("</table></div>");
  });

  it("still escapes untrusted file paths in the header and attributes", () => {
    const html = diffFilesHtml(files);
    expect(html).not.toContain("src/<x>.rs");
    expect(html).toContain("src/&lt;x&gt;.rs");
  });

  it("uses data-file on the file block for comment anchoring", () => {
    expect(diffFilesHtml(files)).toContain('data-file="src/&lt;x&gt;.rs"');
  });

  it("syntax-highlights code by the file's extension (never leaves raw markup)", () => {
    const html = diffFilesHtml(files);
    // rust source → Prism tokens applied to code cells
    expect(html).toContain('class="token');
    // the untrusted `<old>` / `<new>` still never appears as raw markup
    expect(html).not.toContain("<old>");
    expect(html).not.toContain("<new>");
    expect(html).toContain("&lt;"); // escaped (Prism may split it across token spans)
    // no raw tag other than Prism's <span>/the diff <table>/<tr>/<td>/<div> survives
    expect(/<(?!\/?(span|table|tr|td|div)\b)[a-zA-Z]/.test(html)).toBe(false);
  });
});

describe("diffRowsHtml", () => {
  it("preserves the row contract (data-ln + two td.ln columns) with an unknown language", () => {
    const html = diffRowsHtml(files[0].rows, null);
    expect(html).toContain('<tr class="hunk"><td class="ln"></td><td class="ln"></td><td class="code">');
    expect(html).toContain('<tr class="del" data-ln="1"><td class="ln">1</td><td class="ln"></td><td class="code">');
    expect(html).toContain('<tr class="add" data-ln="1"><td class="ln"></td><td class="ln">1</td><td class="code">');
    expect(html).toContain('<tr class="ctx" data-ln="2"><td class="ln">2</td><td class="ln">2</td><td class="code">');
    // unknown lang → escaped, not tokenized
    expect(html).toContain("let x = &lt;old&gt;;");
    expect(html).not.toContain("token");
  });

  it("keeps the exact ln columns intact when a known language tokenizes the code cell", () => {
    const html = diffRowsHtml(files[0].rows, "rust");
    // the ln columns and data-ln are byte-identical; only td.code innerHTML gains tokens
    expect(html).toContain('<tr class="del" data-ln="1"><td class="ln">1</td><td class="ln"></td><td class="code">');
    expect(html).toContain('class="token');
    expect(html).not.toContain("<old>");
  });
});

describe("diff folding + file comments", () => {
  const files = [
    { path: "src/a.js", status: "EDIT", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "x" }] },
  ];

  it("renders every file capped by default (click-to-expand)", () => {
    expect(diffFilesHtml(files)).toContain('class="file capped"');
  });

  it("adds a file-level comment control only when commentable", () => {
    expect(diffFilesHtml(files)).not.toContain("fcmt");
    const html = diffFilesHtml(files, { commentable: true });
    expect(html).toContain('class="fcmt"');
  });
});

describe("dotenv secret masking in diff rows", () => {
  const envFile = [
    {
      path: "config/.env.production",
      status: "EDIT",
      add: 1,
      del: 1,
      rows: [
        { t: "hunk", text: "@@ -1,2 +1,2 @@" },
        { t: "del", o: 1, text: "TOKEN=oldsecret0123456789abcd" },
        { t: "add", n: 1, text: "TOKEN=newsecret9876543210wxyz" },
        { t: "ctx", o: 2, n: 2, text: "HOST=auth.example.com" },
      ],
    },
  ];

  it("masks both old and new secret values as click-to-reveal spoilers", () => {
    const html = diffFilesHtml(envFile);
    expect(html).toContain('class="spoiler"');
    // Neither the removed nor the added secret appears in the rendered HTML text
    // (it rides in the escaped data-secret attribute for reveal-on-click).
    expect(html).toContain('data-secret="oldsecret0123456789abcd"');
    expect(html).toContain('data-secret="newsecret9876543210wxyz"');
    // The visible, non-secret context line stays as-is.
    expect(html).toContain("auth.example.com");
  });

  it("does NOT mask non-dotenv files", () => {
    const plain = [{ ...envFile[0], path: "src/app.js" }];
    expect(diffFilesHtml(plain)).not.toContain("spoiler");
  });

  it("escapes an HTML-bearing masked value (no XSS via data-secret)", () => {
    const hostile = [
      {
        path: ".env",
        status: "EDIT",
        add: 1,
        del: 0,
        rows: [{ t: "add", n: 1, text: 'PASSWORD="<img src=x onerror=alert(1)>"' }],
      },
    ];
    const html = diffFilesHtml(hostile);
    expect(html).toContain("spoiler");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
  });
});
