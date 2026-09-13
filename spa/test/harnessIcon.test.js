import { describe, expect, it } from "vitest";
import { harnessIcon, harnessIconHtml } from "../src/core/harnessIcon.js";

describe("conversation header harness icons", () => {
  it("shares official artwork across the Claude and Codex carrier aliases", () => {
    expect(harnessIcon("claude_adk")).toBe(harnessIcon("claude"));
    expect(harnessIcon("codex_app_server")).toBe(harnessIcon("codex"));
  });

  it("recognizes the remaining harness ids without adding them to creation choices", () => {
    expect(harnessIcon("opencode")).not.toBe(harnessIcon("pi"));
    expect(harnessIconHtml("opencode")).toContain('data-harness-icon="opencode"');
    expect(harnessIconHtml("opencode")).toContain("rail-harness-icon-light");
    expect(harnessIconHtml("opencode")).toContain("rail-harness-icon-dark");
    expect(harnessIconHtml("pi")).toContain('data-harness-icon="pi"');
  });

  it("uses a neutral existing mark for an unknown provider", () => {
    const unknown = harnessIconHtml("future-harness");
    expect(unknown).toContain('data-harness-icon="unknown"');
    expect(unknown).not.toContain("future-harness");
  });

  it("does not treat inherited object property names as providers", () => {
    expect(harnessIconHtml("constructor")).toContain('data-harness-icon="unknown"');
    expect(harnessIconHtml("toString")).toContain('data-harness-icon="unknown"');
  });
});
