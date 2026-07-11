import { describe, it, expect } from "vitest";
import { esc, humanAge } from "../src/core/text.js";

describe("esc", () => {
  it("escapes the HTML-significant characters and stringifies nullish", () => {
    expect(esc("<img src=x onerror=alert(1)>")).toBe("&lt;img src=x onerror=alert(1)&gt;");
    expect(esc("a & b")).toBe("a &amp; b");
    expect(esc(null)).toBe("");
    expect(esc(undefined)).toBe("");
  });

  it("escapes quotes so untrusted text cannot break out of an attribute value", () => {
    // esc() output is interpolated into attributes (data-dir="...", data-tab="...");
    // an unescaped quote would let a hostile filename inject live attributes.
    expect(esc('x" onmouseover="alert(1)')).toBe("x&quot; onmouseover=&quot;alert(1)");
    expect(esc("x' onmouseover='alert(1)")).toBe("x&#39; onmouseover=&#39;alert(1)");
  });
});

describe("humanAge", () => {
  it("renders human-scale ages across the four bands", () => {
    expect(humanAge(5)).toBe("just now");
    expect(humanAge(90)).toBe("1m ago");
    expect(humanAge(7200)).toBe("2h ago");
    expect(humanAge(259200)).toBe("3d ago");
  });

  it("clamps just under each boundary", () => {
    expect(humanAge(59)).toBe("just now");
    expect(humanAge(3599)).toBe("59m ago");
    expect(humanAge(86399)).toBe("23h ago");
  });
});
