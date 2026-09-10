import { describe, it, expect } from "vitest";
import {
  previewModeFor,
  previewHasSourceToggle,
  decodeBase64Text,
  filesTreeHtml,
  sourcePreviewHtml,
  mediaPreviewHtml,
  previewPlaceholderHtml,
  shouldMaskDotenv,
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
    expect(previewModeFor("audio/mpeg", false)).toBe("audio");
    expect(previewModeFor("video/mp4", false)).toBe("video");
    expect(previewModeFor("application/octet-stream", false)).toBe("binary");
    expect(previewModeFor("application/pdf", false)).toBe("binary");
    expect(previewModeFor("text/plain", false)).toBe("source");
    expect(previewModeFor("application/json", false)).toBe("source");
  });

  it("demotes truncated image/html/svg to a size placeholder", () => {
    expect(previewModeFor("text/html", true)).toBe("toolarge");
    expect(previewModeFor("image/svg+xml", true)).toBe("toolarge");
    expect(previewModeFor("image/png", true)).toBe("toolarge");
    expect(previewModeFor("audio/wav", true)).toBe("toolarge");
    expect(previewModeFor("video/webm", true)).toBe("toolarge");
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

  it("gives every name a box of its own to truncate inside", () => {
    // The tree column is a fixed width that gives ground rather than growing,
    // so a name with no break opportunity has to ellipsize — which needs an
    // element, not a bare text node in the row's flex line.
    const html = filesTreeHtml("src", [
      { kind: "dir", name: "core" },
      { kind: "file", name: "aVeryLongUnbrokenFileName.module.test.js", size: 12 },
      { kind: "symlink", name: "link" },
    ]);
    expect(html.match(/class="fname"/g)).toHaveLength(3);
    expect(html).toContain('<span class="fname">aVeryLongUnbrokenFileName.module.test.js</span>');
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
  it("syntax-highlights source by extension with a numbered row per line", () => {
    const html = sourcePreviewHtml("bridge/src/app.rs", "fn main() { let x = 1; }");
    expect(html).toContain('<div class="fsrc"><table>');
    expect(html).toContain('<td class="fsrc-ln">1</td>');
    expect(html).toContain('class="token');
    expect(html).toContain("</table></div>");
  });

  it("renders stable line numbers including blank lines", () => {
    const html = sourcePreviewHtml("notes.txt", "first\n\nthird");
    expect(html.match(/class="fsrc-ln"/g)).toHaveLength(3);
    expect(html).toContain('<td class="fsrc-ln">3</td>');
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
    expect(/<(?!\/?(span|div|table|tr|td|code)\b)[a-zA-Z]/.test(html)).toBe(false);
  });
});

describe("mediaPreviewHtml", () => {
  it("renders native audio and video controls", () => {
    expect(mediaPreviewHtml("audio", "audio/mpeg", "SUQz")).toContain('<audio class="fmedia faudio" controls');
    expect(mediaPreviewHtml("video", "video/mp4", "AAAA")).toContain('<video class="fmedia fvideo" controls');
  });

  it("escapes a hostile MIME hint", () => {
    expect(mediaPreviewHtml("audio", 'audio/mpeg" onerror="alert(1)', "SUQz")).not.toContain('onerror="alert(1)"');
  });
});

describe("shouldMaskDotenv", () => {
  it("masks a dotenv file only in its source view", () => {
    expect(shouldMaskDotenv(".env", "source", false)).toBe(true);
    expect(shouldMaskDotenv("config/.env.local", "source", false)).toBe(true);
    // a source override on a rendered type still masks
    expect(shouldMaskDotenv(".env", "markdown", true)).toBe(true);
  });

  it("never masks non-dotenv files or binary/toolarge previews", () => {
    expect(shouldMaskDotenv("src/app.js", "source", false)).toBe(false);
    expect(shouldMaskDotenv(".env", "binary", false)).toBe(false);
    expect(shouldMaskDotenv(".env", "toolarge", false)).toBe(false);
  });
});

describe("previewPlaceholderHtml", () => {
  it("renders idle and error messages as a quiet centered block", () => {
    const idle = previewPlaceholderHtml("idle", "No file open");
    expect(idle).toContain('class="fpidle"');
    expect(idle).toContain('class="fpidle-msg">No file open<');
    const error = previewPlaceholderHtml("error", "cannot read: nope");
    expect(error).toContain("cannot read: nope");
    expect(error).toContain("fpidle-error");
  });

  // An empty pane that only says it is empty leaves the reader looking for the
  // control that fills it — which, below the stacking width, is behind a drawer
  // handle rather than in view.
  it("carries a hint beside the message when one is given", () => {
    const idle = previewPlaceholderHtml("idle", "No file open", "Choose a file from the tree.");
    expect(idle).toContain('class="fpidle-hint">Choose a file from the tree.<');
    expect(previewPlaceholderHtml("idle", "No file open")).not.toContain("fpidle-hint");
  });

  it("escapes placeholder messages (error text carries repo-derived paths)", () => {
    expect(previewPlaceholderHtml("error", "<img src=x>")).not.toContain("<img");
    expect(previewPlaceholderHtml("idle", "ok", "<img src=x>")).not.toContain("<img");
  });

  it("renders loading as a throbber", () => {
    expect(previewPlaceholderHtml("loading")).toContain('class="throbber"');
  });
});
