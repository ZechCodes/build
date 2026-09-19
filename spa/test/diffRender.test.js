import { describe, it, expect } from "vitest";
import { diffFilesHtml, diffRowsHtml, diffStackEntries, diffStackHtml } from "../src/core/diffRender.js";
import { createFileFolds, fileKey } from "../src/core/diff.js";

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
    expect(html).toContain('<tr class="hunk" aria-rowindex="1">');
    expect(html).toContain('<tr class="del" aria-rowindex="2" data-ln="1" data-side="old"');
    expect(html).toContain('<tr class="add" aria-rowindex="3" data-ln="1" data-side="new"');
    expect(html).toContain('<tr class="ctx" aria-rowindex="4" data-ln="2" data-side="new"');
  });

  it("wraps the table in a .dscroll box so the code scrolls under a fixed header", () => {
    const html = diffFilesHtml(files);
    expect(html).toContain('<div class="dscroll" data-row-count="4"><table aria-rowcount="4">');
    expect(html).toContain("</table></div>");
  });

  it("still escapes untrusted file paths in the header and attributes", () => {
    const html = diffFilesHtml(files);
    expect(html).not.toContain("src/<x>.rs");
    expect(html).toContain("src/&lt;x&gt;.rs");
  });

  it("names the file block with its key, and nothing else, for the anchoring to read", () => {
    const html = diffFilesHtml(files);
    expect(html).toContain('data-key="EDIT:src/&lt;x&gt;.rs"');
    expect(html).not.toContain("data-file=");
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
  it("keeps a very long line intact without sending it through syntax highlighting", () => {
    const text = `const payload = "${"x".repeat(25_000)}";`;
    const html = diffRowsHtml([{ t: "add", n: 1, text }], "javascript");
    expect(html).toContain(text.replaceAll('"', "&quot;"));
    expect(html).not.toContain('class="token');
  });

  it("preserves the row contract (data-ln + two td.ln columns) with an unknown language", () => {
    const html = diffRowsHtml(files[0].rows, null);
    expect(html).toContain('<tr class="hunk" aria-rowindex="1"><td class="ln"></td><td class="ln"></td><td class="code">');
    expect(html).toContain('<tr class="del" aria-rowindex="2" data-ln="1" data-side="old" data-old-line="1" data-new-line=""><td class="ln">1</td><td class="ln"></td><td class="code">');
    expect(html).toContain('<tr class="add" aria-rowindex="3" data-ln="1" data-side="new" data-old-line="" data-new-line="1"><td class="ln"></td><td class="ln">1</td><td class="code">');
    expect(html).toContain('<tr class="ctx" aria-rowindex="4" data-ln="2" data-side="new" data-old-line="2" data-new-line="2"><td class="ln">2</td><td class="ln">2</td><td class="code">');
    // unknown lang → escaped, not tokenized
    expect(html).toContain("let x = &lt;old&gt;;");
    expect(html).not.toContain("token");
  });

  it("keeps the exact ln columns intact when a known language tokenizes the code cell", () => {
    const html = diffRowsHtml(files[0].rows, "rust");
    // anchor metadata and line columns remain intact; only td.code innerHTML gains tokens
    expect(html).toContain('<tr class="del" aria-rowindex="2" data-ln="1" data-side="old" data-old-line="1" data-new-line=""><td class="ln">1</td><td class="ln"></td><td class="code">');
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

// The header's controls are icons from the app's own pack (core/icons.js),
// never typed glyphs: an arrow drawn in the body font sits at whatever size and
// weight the font gives it, which is why they read as too small beside the
// path.
describe("the file header's controls", () => {
  const files = [{ path: "src/a.js", status: "EDIT", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "x" }] }];

  it("opens the file with an accessible icon-only button", () => {
    const html = diffFilesHtml(files, { openable: true });
    expect(html).toContain('aria-label="Open this file in Files"');
    expect(html).not.toContain("<span>Open File</span>");
    expect(html).not.toContain("↗");
    expect(html).toContain("lucide-external-link");
  });

  it("says comment with a comment icon rather than a pencil", () => {
    const html = diffFilesHtml(files, { commentable: true });
    expect(html).not.toContain("✎");
    expect(html).toContain("lucide-message-square");
  });

  it("offers an Approve toggle where it used to offer a Viewed checkbox", () => {
    const html = diffFilesHtml(files, { approvable: true });
    expect(html).not.toContain("Viewed");
    expect(html).toContain("Approve");
    expect(html).toContain('class="fapprove"');
    expect(html).toContain('aria-pressed="false"');
    expect(html).not.toContain("<span>Approve</span>");
  });

  it("marks an approved file's toggle pressed", () => {
    const html = diffFilesHtml(files, { approvable: true, approved: new Set(["src/a.js"]) });
    expect(html).toContain('aria-pressed="true"');
  });

  it("collapses an approved file, and caps one nobody has approved", () => {
    const two = [...files, { path: "src/b.js", status: "EDIT", add: 1, del: 0, rows: [] }];
    const html = diffFilesHtml(two, { approvable: true, approved: new Set(["src/a.js"]) });
    expect(html).toMatch(/<div class="file collapsed" data-key="EDIT:src\/a\.js"/);
    expect(html).toMatch(/<div class="file capped" data-key="EDIT:src\/b\.js"/);
  });

  it("puts a selection checkbox ahead of the path when the surface selects files", () => {
    const html = diffFilesHtml(files, { selectable: true });
    expect(html).toContain('class="fselect-box"');
    expect(html.indexOf("fselect-box")).toBeLessThan(html.indexOf("fpath"));
  });

  it("ticks the selection checkbox of a selected file", () => {
    const html = diffFilesHtml(files, { selectable: true, selected: new Set(["src/a.js"]) });
    expect(html).toMatch(/fselect-box[^>]*checked/);
  });

  it("places a valid edit time after the diffstat and omits missing or invalid times", () => {
    const edited = { ...files[0], editedAt: Date.now() - 60_000 };
    const html = diffFilesHtml([edited]);
    expect(html).toContain('class="fedited"');
    expect(html.indexOf('class="pm"')).toBeLessThan(html.indexOf('class="fedited"'));
    expect(diffFilesHtml(files)).not.toContain('class="fedited"');
    expect(diffFilesHtml([{ ...files[0], editedAt: 9e15 }])).not.toContain('class="fedited"');
  });

  it("offers no selection checkbox where the surface does not select files", () => {
    expect(diffFilesHtml(files)).not.toContain("fselect-box");
  });
});

describe("diffFilesHtml re-review options", () => {
  const files = [
    { path: "a.js", status: "EDIT", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "x" }] },
    { path: "b.js", status: "EDIT", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "y" }] },
  ];

  it("is byte-identical to the old output when no new options are passed", () => {
    expect(diffFilesHtml(files, { commentable: true })).toBe(
      diffFilesHtml(files, { commentable: true, changedSince: null, approved: null, approvable: false }),
    );
  });

  it("emits no re-review markup by default", () => {
    const html = diffFilesHtml(files);
    expect(html).not.toContain("fchanged");
    expect(html).not.toContain("fapprove");
  });

  it("marks only the changed paths with a changed-since chip", () => {
    const html = diffFilesHtml(files, { changedSince: new Set(["a.js"]) });
    expect((html.match(/fchanged/g) || []).length).toBe(1);
    expect(html).toContain("changed since your review");
  });

  it("renders an Approve toggle per file when approvable", () => {
    const html = diffFilesHtml(files, { approvable: true });
    expect((html.match(/class="fapprove"/g) || []).length).toBe(2);
  });

  it("collapses (not caps) an approved file and presses its toggle", () => {
    const html = diffFilesHtml(files, { approvable: true, approved: new Set(["a.js"]) });
    expect(html).toMatch(/<div class="file collapsed" data-key="EDIT:a\.js"/);
    expect(html).toMatch(/<div class="file capped" data-key="EDIT:b\.js"/);
    expect(html).toContain('data-key="EDIT:a.js" aria-pressed="true"');
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

describe("per-file ⋯ menu (staging is gone; discard moved here)", () => {
  const files = [{ path: "src/a.js", status: "EDIT", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "x" }] }];

  it("renders no menu control unless the changeset offers one", () => {
    expect(diffFilesHtml(files)).not.toContain("fmenu");
  });

  it("renders no stage checkbox anywhere — commit is commit-all", () => {
    expect(diffFilesHtml(files, { commentable: true, fileMenu: {} })).not.toContain("stagebox");
  });

  it("puts discard behind the file header's ⋯", () => {
    const html = diffFilesHtml(files, { fileMenu: { openPath: "src/a.js" } });
    expect(html).toContain('class="fmenu"');
    expect(html).toContain("gitdiscard");
    expect(html).toContain('data-key="EDIT:src/a.js"');
  });

  it("keeps the menu shut until its own file's ⋯ is open", () => {
    const html = diffFilesHtml(files, { fileMenu: {} });
    expect(html).toContain('class="fmenu"');
    expect(html).not.toContain("gitdiscard");
  });

  it("arms the discard with the two-click confirm label", () => {
    const html = diffFilesHtml(files, { fileMenu: { openPath: "src/a.js", pendingConfirm: "discard:src/a.js" } });
    expect(html).toContain("Discard changes?");
    expect(html).toContain("armed");
  });

  it("escapes the path in the menu's data attributes", () => {
    const evil = [{ path: '"><img src=x>', status: "EDIT", add: 1, del: 0, rows: [] }];
    expect(diffFilesHtml(evil, { fileMenu: { openPath: '"><img src=x>' } })).not.toContain("<img");
  });
});

describe("diffStackHtml — collapse, never hide", () => {
  const source = { path: "src/main.py", status: "EDIT", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "x" }] };
  const lock = { path: "uv.lock", status: "EDIT", add: 9, del: 9, rows: [{ t: "add", n: 1, text: "y" }] };
  const meta = { path: ".build/state.json", status: "EDIT", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "z" }] };

  it("renders source files as full stacked diffs", () => {
    const html = diffStackHtml([source]);
    expect(html).toContain('data-key="EDIT:src/main.py"');
    expect(html).not.toContain("noisegroup");
  });

  it("collapses noise into one counted group at the bottom instead of dropping it", () => {
    const html = diffStackHtml([source, lock, meta]);
    expect(html).toContain("noisegroup");
    expect(html).toContain("2 generated files");
    // the group sits below the source stack, and its diffs are not rendered yet
    expect(html.indexOf("noisegroup")).toBeGreaterThan(html.indexOf('data-key="EDIT:src/main.py"'));
    expect(html).not.toContain('data-key="EDIT:uv.lock"');
  });

  it("renders the noise diffs once the group is expanded", () => {
    const html = diffStackHtml([source, lock], { noiseExpanded: true });
    expect(html).toContain('data-key="EDIT:uv.lock"');
    expect(html).toContain("noisegroup open");
  });

  it("shows the group even when every file is noise", () => {
    const html = diffStackHtml([lock, meta]);
    expect(html).toContain("2 generated files");
  });

  it("passes the file options through to every rendered file", () => {
    expect(diffStackHtml([source], { commentable: true })).toContain("fcmt");
    expect(diffStackHtml([lock], { commentable: true, noiseExpanded: true })).toContain("fcmt");
  });

  it("says so when there is nothing at all", () => {
    expect(diffStackHtml([])).toContain("No file changes");
  });
});

// The fold a file wears is a function of the reader's state, not of the class
// list a press left behind: `expanded` and `collapsed` are sets of file keys,
// and `viewed` still shuts a file the reader ticked off.
describe("folds the reader owns", () => {
  const files = [
    { path: "a.js", status: "EDIT", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "x" }] },
    { path: "b.js", status: "EDIT", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "y" }] },
  ];
  const keyA = fileKey(files[0]);

  it("names every file with its key, beside the path the comments anchor to", () => {
    const html = diffFilesHtml(files);
    expect(html).toContain(`data-key="${keyA}"`);
  });

  it("caps a file the reader has not opened", () => {
    expect(diffFilesHtml(files)).toContain('class="file capped" data-key="EDIT:a.js"');
  });

  it("expands the file whose key the reader opened, and says so in its class alone", () => {
    const folds = createFileFolds();
    folds.openBody(keyA);
    const html = diffFilesHtml(files, { folds });
    expect(html).toContain('class="file" data-key="EDIT:a.js"');
    expect(html).not.toContain("data-expanded");
    expect(html).toContain('class="file capped" data-key="EDIT:b.js"');
  });

  it("collapses the file the reader shut", () => {
    const folds = createFileFolds();
    folds.press(keyA);
    expect(diffFilesHtml(files, { folds })).toContain('class="file collapsed" data-key="EDIT:a.js"');
  });

  it("collapses an approved file, with or without a reader's folds behind it", () => {
    expect(diffFilesHtml(files, { approved: new Set(["a.js"]) })).toContain('class="file collapsed" data-key="EDIT:a.js"');
    const html = diffFilesHtml(files, { folds: createFileFolds(), approved: new Set(["a.js"]) });
    expect(html).toContain('class="file collapsed" data-key="EDIT:a.js"');
  });

  it("opens an approved file the reader asked to see again", () => {
    const approved = new Set(["a.js"]);
    const folds = createFileFolds();
    folds.press(keyA, { approved });
    const html = diffFilesHtml(files, { folds, approved, approvable: true });
    expect(html).toContain('class="file" data-key="EDIT:a.js"');
    expect(html).toContain('data-key="EDIT:a.js" aria-pressed="true"');
  });
});

// The stack is a keyed list: every block a repaint can move — a file, the
// noise group — has a name of its own, so patching one leaves the rest
// standing. diffStackHtml is those entries joined, and nothing else.
describe("diffStackEntries", () => {
  const source = { path: "src/main.py", status: "EDIT", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "x" }] };
  const other = { path: "src/other.py", status: "EDIT", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "y" }] };
  const lock = { path: "uv.lock", status: "EDIT", add: 9, del: 9, rows: [{ t: "add", n: 1, text: "z" }] };

  it("names every file entry by its file key", () => {
    const entries = diffStackEntries([source, other]);
    expect(entries.map((entry) => entry.key)).toEqual([fileKey(source), fileKey(other)]);
  });

  it("gives the noise group a name of its own, after the files", () => {
    const entries = diffStackEntries([source, lock]);
    expect(entries.map((entry) => entry.key)).toEqual([fileKey(source), "noise"]);
    expect(entries[1].html).toContain("noisegroup");
  });

  it("says so, under one name, when there is nothing at all", () => {
    const entries = diffStackEntries([]);
    expect(entries).toHaveLength(1);
    expect(entries[0].html).toContain("No file changes");
  });

  it("keeps the keys unique so a list can be patched by them", () => {
    const entries = diffStackEntries([source, other, lock], { noiseExpanded: true });
    expect(new Set(entries.map((entry) => entry.key)).size).toBe(entries.length);
  });

  it("is what diffStackHtml is made of", () => {
    for (const options of [{}, { noiseExpanded: true }, { commentable: true }]) {
      expect(diffStackEntries([source, lock], options).map((entry) => entry.html).join("")).toBe(
        diffStackHtml([source, lock], options),
      );
    }
  });
});

// A diff says what changed; the file says what it is now. One control in each
// file's head carries the reader from one to the other.
describe("the way into the Files view", () => {
  const files = [
    {
      path: "src/a.js",
      status: "EDIT",
      add: 1,
      del: 0,
      rows: [
        { t: "hunk", text: "@@ -12,2 +12,3 @@" },
        { t: "ctx", o: 12, n: 12, text: "keep" },
        { t: "add", n: 13, text: "x" },
      ],
    },
  ];

  it("offers nothing where the surface cannot open a file", () => {
    expect(diffFilesHtml(files)).not.toContain("data-open-file");
  });

  it("carries the path and the first line the diff touches", () => {
    const html = diffFilesHtml(files, { openable: true });
    expect(html).toContain('data-open-file="src/a.js"');
    expect(html).toContain('data-new-line="12"');
  });

  it("escapes the path it carries", () => {
    const evil = [{ path: '"><img src=x>', status: "EDIT", add: 0, del: 0, rows: [] }];
    expect(diffFilesHtml(evil, { openable: true })).not.toContain("<img");
  });
});
