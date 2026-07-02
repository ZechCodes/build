import { describe, it, expect } from "vitest";
import {
  modelOptionsHtml,
  effortOptionsHtml,
  effortSupported,
  modelParams,
} from "../src/core/modelPicker.js";

// The selectors are fed by the bridge's models.list RPC (the catalog ships with
// the bridge, never the UI). "Harness default" — empty value — must always be
// offered: it means the user's own Claude Code config decides.

const MODELS = [
  { id: "claude-opus-4-8", label: "Claude Opus 4.8", supports_effort: true },
  { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", supports_effort: false },
];
const EFFORTS = ["low", "medium", "high", "xhigh", "max"];

describe("modelOptionsHtml", () => {
  it("offers harness default first, then the catalog, marking the selection", () => {
    const html = modelOptionsHtml(MODELS, "claude-opus-4-8");
    expect(html.indexOf('value=""')).toBeGreaterThan(-1);
    expect(html.indexOf('value=""')).toBeLessThan(html.indexOf("claude-opus-4-8"));
    expect(html).toContain('value="claude-opus-4-8" selected');
    expect(html).toContain("Claude Haiku 4.5");
  });

  it("keeps a selection that is not in the catalog selectable (newer model)", () => {
    const html = modelOptionsHtml(MODELS, "claude-opus-5");
    expect(html).toContain('value="claude-opus-5" selected');
  });

  it("escapes labels", () => {
    const html = modelOptionsHtml([{ id: "m", label: "<b>x</b>", supports_effort: true }], "");
    expect(html).not.toContain("<b>");
  });
});

describe("effortSupported", () => {
  it("is true for harness default and unknown models, false for no-effort catalog models", () => {
    expect(effortSupported(MODELS, "")).toBe(true);
    expect(effortSupported(MODELS, "claude-opus-4-8")).toBe(true);
    expect(effortSupported(MODELS, "claude-opus-5")).toBe(true);
    expect(effortSupported(MODELS, "claude-haiku-4-5")).toBe(false);
  });
});

describe("effortOptionsHtml", () => {
  it("offers default first and marks the selection", () => {
    const html = effortOptionsHtml(EFFORTS, "xhigh");
    expect(html.indexOf('value=""')).toBeLessThan(html.indexOf("xhigh"));
    expect(html).toContain('value="xhigh" selected');
  });
});

describe("modelParams", () => {
  it("omits empties so the harness default stays the default", () => {
    expect(modelParams(MODELS, "", "")).toEqual({});
    expect(modelParams(MODELS, "claude-opus-4-8", "")).toEqual({
      model: "claude-opus-4-8",
    });
    expect(modelParams(MODELS, "", "high")).toEqual({ effort: "high" });
  });

  it("passes model and effort together", () => {
    expect(modelParams(MODELS, "claude-opus-4-8", "xhigh")).toEqual({
      model: "claude-opus-4-8",
      effort: "xhigh",
    });
  });

  it("drops effort for catalog models that do not support it", () => {
    expect(modelParams(MODELS, "claude-haiku-4-5", "high")).toEqual({
      model: "claude-haiku-4-5",
    });
  });
});
