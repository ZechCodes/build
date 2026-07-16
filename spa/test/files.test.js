import { describe, it, expect } from "vitest";
import {
  previewModeFor,
  previewHasSourceToggle,
  decodeBase64Text,
  filesTreeHtml,
  sourcePreviewHtml,
  previewPlaceholderHtml,
} from "../src/views/files.js";
import { esc } from "../src/core/text.js";

const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

describe("previewModeFor", () => {
  it("maps the mime table to render modes", () => {
    expect(previewModeFor("text/markdown", false)).toBe("markdown");
    expect(previewModeFor("text/html", false)).toBe("html");
    expect(previewModeFor("image/svg+xml", false)).toBe("svg");
    expect(previewModeFor("image/png", false)).toBe("image");
    expect(previewModeFor("image/jpeg", false)).toBe("image");
    expect(previewModeFor("application/octet-stream", false)).toBe("binary");
    expect(previewModeFor("application/pdf", false)).toBe("binary");
    expect(previewModeFor("text/plain", false)).toBe("source");
    expect(previewModeFor("application/json", false)).toBe("source");
  });

  it("demotes truncated image/html/svg to a size placeholder", () => {
    expect(previewModeFor("text/html", true)).toBe("toolarge");
    expect(previewModeFor("image/svg+xml", true)).toBe("toolarge");
    expect(previewModeFor("image/png", true)).toBe("toolarge");
  });

  it("keeps truncated markdown/source renderable (partial + notice)", () => {
    expect(previewModeFor("text/markdown", true)).toBe("markdown");
    expect(previewModeFor("text/plain", true)).toBe("source");
    expect(previewModeFor("application/octet-stream", true)).toBe("binary");
  });
});

describe("previewHasSourceToggle", () => {
  it("offers a source toggle for rendered types only", () => {
    expect(previewHasSourceToggle("markdown")).toBe(true);
    expect(previewHasSourceToggle("html")).toBe(true);
    expect(previewHasSourceToggle("svg")).toBe(true);
    expect(previewHasSourceToggle("image")).toBe(false);
    expect(previewHasSourceToggle("binary")).toBe(false);
    expect(previewHasSourceToggle("source")).toBe(false);
  });
});

// The tree pane interpolates untrusted repo file/dir names into row HTML —
// this exercises the REAL row-building path (spec §9: an `<img src=x onerror>`
// filename must render inert), not esc() in isolation.
describe("filesTreeHtml", () => {
  it("escapes hostile file/dir/symlink names in the rendered rows", () => {
    const html = filesTreeHtml("", [
      { kind: "dir", name: "<img src=x onerror=alert(1)>" },
      { kind: "file", name: "<script>alert(2)</script>.txt", size: 3 },
      { kind: "symlink", name: "<svg onload=alert(3)>" },
    ]);
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<svg");
    expect(html).toContain("&lt;img src=x onerror=alert(1)&gt;");
    expect(html).toContain("&lt;script&gt;alert(2)&lt;/script&gt;.txt");
  });

  it("a quoted filename cannot break out of the data-dir/data-file attributes", () => {
    const html = filesTreeHtml("", [
      { kind: "dir", name: 'd" onmouseover="alert(1)' },
      { kind: "file", name: 'f" onfocus="alert(2)', size: 1 },
    ]);
    expect(html).not.toContain('onmouseover="alert(1)"');
    expect(html).not.toContain('onfocus="alert(2)"');
    expect(html).toContain("&quot;");
  });

  it("escapes the breadcrumb directory and renders the up-row only below the root", () => {
    const nested = filesTreeHtml('<b>evil</b>/"sub', [{ kind: "file", name: "a.txt", size: 1 }]);
    expect(nested).not.toContain("<b>evil</b>");
    expect(nested).toContain("fup");
    const root = filesTreeHtml("", []);
    expect(root).not.toContain("fup");
    expect(root).toContain("Empty directory.");
  });
});

describe("decodeBase64Text", () => {
  it("round-trips UTF-8 through base64", () => {
    expect(decodeBase64Text(b64("# Hello — world"))).toBe("# Hello — world");
    expect(decodeBase64Text("")).toBe("");
  });

  it("a source view of a hostile filename's contents stays inert once escaped", () => {
    const source = decodeBase64Text(b64("<img src=x onerror=alert(1)>"));
    // The view wraps decoded source in esc(); the escaped form carries no live tag.
    const escaped = esc(source);
    expect(escaped).not.toContain("<img");
    expect(escaped).toContain("&lt;img");
  });
});

describe("sourcePreviewHtml", () => {
  it("syntax-highlights source by the file's extension inside a <pre class=fsrc>", () => {
    const html = sourcePreviewHtml("bridge/src/app.rs", "fn main() { let x = 1; }");
    expect(html).toContain('<pre class="fsrc"><code>');
    expect(html).toContain('class="token');
    expect(html).toContain("</code></pre>");
  });

  it("escapes source of an unknown extension and never emits a live tag", () => {
    const html = sourcePreviewHtml("notes.unknownext", "<img src=x onerror=alert(1)>");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
    expect(html).not.toContain("token");
  });

  it("keeps a hostile payload inert even under a real grammar", () => {
    const html = sourcePreviewHtml("evil.js", `<script>alert(1)</script>`);
    expect(html).not.toContain("<script>");
    expect(/<(?!\/?(span|pre|code)\b)[a-zA-Z]/.test(html)).toBe(false);
  });
});

describe("previewPlaceholderHtml", () => {
  it("renders idle and error messages as a quiet centered line", () => {
    expect(previewPlaceholderHtml("idle", "Select a file to preview.")).toBe(
      '<div class="fpidle">Select a file to preview.</div>',
    );
    expect(previewPlaceholderHtml("error", "cannot read: nope")).toContain("cannot read: nope");
  });

  it("escapes placeholder messages (error text carries repo-derived paths)", () => {
    expect(previewPlaceholderHtml("error", "<img src=x>")).not.toContain("<img");
  });

  it("renders loading as a throbber", () => {
    expect(previewPlaceholderHtml("loading")).toContain('class="throbber"');
  });
});
