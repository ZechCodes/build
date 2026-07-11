import { describe, it, expect } from "vitest";
import { previewModeFor, previewHasSourceToggle, decodeBase64Text } from "../src/views/files.js";
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
