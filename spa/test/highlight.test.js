import { describe, it, expect } from "vitest";
import { langForPath, highlightCode } from "../src/core/highlight.js";

describe("langForPath", () => {
  it("maps known source extensions to Prism language ids", () => {
    expect(langForPath("src/app.js")).toBe("javascript");
    expect(langForPath("src/App.jsx")).toBe("jsx");
    expect(langForPath("src/app.ts")).toBe("typescript");
    expect(langForPath("src/App.tsx")).toBe("tsx");
    expect(langForPath("bridge/main.py")).toBe("python");
    expect(langForPath("bridge/src/app.rs")).toBe("rust");
    expect(langForPath("src/styles.css")).toBe("css");
    expect(langForPath("index.html")).toBe("markup");
    expect(langForPath("package.json")).toBe("json");
    expect(langForPath("run.sh")).toBe("bash");
    expect(langForPath("ci.yaml")).toBe("yaml");
    expect(langForPath("ci.yml")).toBe("yaml");
    expect(langForPath("Cargo.toml")).toBe("toml");
    expect(langForPath("README.md")).toBe("markdown");
  });

  it("is case-insensitive on the extension", () => {
    expect(langForPath("SRC/App.RS")).toBe("rust");
  });

  it("returns null for unknown or extension-less paths", () => {
    expect(langForPath("Makefile")).toBeNull();
    expect(langForPath("data.bin")).toBeNull();
    expect(langForPath("")).toBeNull();
    expect(langForPath(null)).toBeNull();
    expect(langForPath("a.dir/file")).toBeNull();
  });
});

describe("highlightCode", () => {
  it("wraps a known-language snippet in .token spans", () => {
    const out = highlightCode("fn main() { let x = 1; }", "rust");
    expect(out).toContain('class="token');
    expect(out).toContain("let");
  });

  it("escapes and does not tokenize an unknown language", () => {
    const out = highlightCode("let x = <old>;", null);
    expect(out).toBe("let x = &lt;old&gt;;");
    expect(out).not.toContain("token");
  });

  it("escapes an unknown-language hostile payload fully (all of < > \" ')", () => {
    const out = highlightCode(`<img src=x onerror="alert('1')">`, null);
    expect(out).not.toContain("<img");
    expect(out).toContain("&lt;img");
    expect(out).toContain("&gt;");
    expect(out).toContain("&quot;");
    expect(out).toContain("&#39;");
  });

  it("never emits a raw opening tag from a hostile payload even under a real grammar", () => {
    // Prism.highlight escapes `<` and `&` in token text — enough that no attacker
    // markup survives as a live tag. (A bare `>` in text content is inert.)
    const out = highlightCode(`<img src=x onerror="alert(1)">`, "javascript");
    expect(out).not.toContain("<img");
    expect(out).toContain("&lt;"); // the payload's `<` is escaped (may be span-wrapped)
    // no raw tag other than Prism's own <span> wrappers survives in the output
    expect(/<(?!\/?span\b)[a-zA-Z]/.test(out)).toBe(false);
  });

  it("treats null/undefined text as empty", () => {
    expect(highlightCode(null, "javascript")).toBe("");
    expect(highlightCode(undefined, null)).toBe("");
  });
});
