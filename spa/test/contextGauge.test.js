// @vitest-environment jsdom
// How full the agent's context is, beside the paperclip.
//
// The figure comes off the agent's digest (`last_context_tokens`, wire 1.10)
// against where the chat compacts (`compact_at_tokens`), or the window its
// model has when it never does. It takes no space when either is missing,
// and it is written onto a node of its own: the textarea beside it is never
// touched, so a push of the agent record mid-sentence keeps the caret.

import { describe, it, expect, beforeEach } from "vitest";
import { composerHtml, composerPartIds } from "../src/core/composer.js";
import { contextGauge, contextWindowOf, mountContextGauge } from "../src/core/contextGauge.js";

const IDS = { input: "ti", send: "ts", hint: "th" };
const CATALOG = {
  default_provider: "claude_adk",
  providers: [
    { id: "claude_adk", label: "Claude Code", models: [{ id: "claude-opus-5", label: "Claude Opus 5" }], efforts: [] },
    { id: "codex", label: "Codex", models: [{ id: "gpt", label: "GPT" }], efforts: [] },
  ],
};
const claude = (tokens, extra = {}) => ({ id: "a1", provider: "claude_adk", last_context_tokens: tokens, ...extra });

const gaugeNode = () => document.querySelector(`#${composerPartIds(IDS.input).gauge}`);
const textarea = () => document.querySelector(`#${IDS.input}`);

describe("contextWindowOf", () => {
  it("is 1M for the Claude harnesses, headless and TUI", () => {
    expect(contextWindowOf({ provider: "claude_adk" }, CATALOG)).toBe(1000000);
    expect(contextWindowOf({ provider: "claude" }, CATALOG)).toBe(1000000);
  });

  it("prefers a window the catalog names for the agent's model", () => {
    const catalog = {
      providers: [{ id: "codex", models: [{ id: "gpt", label: "GPT", context_window: 400000 }] }],
    };
    expect(contextWindowOf({ provider: "codex", active_model: "gpt" }, catalog)).toBe(400000);
  });

  it("is null for a harness with no known window", () => {
    expect(contextWindowOf({ provider: "codex", active_model: "gpt" }, CATALOG)).toBeNull();
  });
});

describe("contextGauge", () => {
  it("measures against the compaction threshold when the chat compacts", () => {
    const gauge = contextGauge(claude(190000, { compact_at_tokens: 200000 }), CATALOG);
    expect(gauge).toMatchObject({ text: "95%", percent: 95, step: "warning" });
    expect(gauge.title).toBe("190k of 200k tokens (compacts at 200k), as of its last turn");
  });

  it("measures against the model's window when the chat never compacts", () => {
    const gauge = contextGauge(claude(612000, { compact_at_tokens: 0 }), CATALOG);
    expect(gauge).toMatchObject({ text: "61%", step: "accent" });
    expect(gauge.title).toBe("612k of 1M window, as of its last turn");
  });

  it("names a threshold the context has passed", () => {
    expect(contextGauge(claude(1200000), CATALOG).title).toBe("1.2M of 1M window, as of its last turn");
    expect(contextGauge(claude(210000, { compact_at_tokens: 200000 }), CATALOG).text).toBe("105%");
  });

  it("reads the percentage off the record", () => {
    expect(contextGauge(claude(612000), CATALOG)).toMatchObject({ text: "61%", percent: 61 });
  });

  it("steps dim below 50%, accent from 50 to 80%, warning above", () => {
    expect(contextGauge(claude(490000), CATALOG).step).toBe("dim");
    expect(contextGauge(claude(500000), CATALOG).step).toBe("accent");
    expect(contextGauge(claude(800000), CATALOG).step).toBe("accent");
    expect(contextGauge(claude(810000), CATALOG).step).toBe("warning");
  });

  it("falls back to the window for a digest from before compaction", () => {
    expect(contextGauge(claude(612000), CATALOG).title).toBe("612k of 1M window, as of its last turn");
  });

  it("is absent without the field, before a turn, or without a window", () => {
    expect(contextGauge({ id: "a1", provider: "claude_adk" }, CATALOG)).toBeNull();
    expect(contextGauge(claude(null), CATALOG)).toBeNull();
    expect(contextGauge({ provider: "codex", last_context_tokens: 5000 }, CATALOG)).toBeNull();
    expect(contextGauge({ provider: "codex", last_context_tokens: 5000, compact_at_tokens: 0 }, CATALOG)).toBeNull();
    expect(contextGauge(null, CATALOG)).toBeNull();
  });
});

describe("the gauge on the composer", () => {
  let gauge;
  beforeEach(() => {
    document.body.innerHTML = composerHtml({
      inputId: IDS.input,
      sendId: IDS.send,
      hintId: IDS.hint,
      placeholder: "Say",
      attachable: true,
      modelMenu: true,
    });
    gauge = mountContextGauge(document.body, { ids: IDS });
  });

  it("stands beside the paperclip, hidden until there is a figure", () => {
    // The file input between them is hidden and takes no space.
    expect(document.querySelector(".composer-actions > .composer-gauge + .composer-file + .composer-attach")).not.toBeNull();
    expect(gaugeNode().hidden).toBe(true);
  });

  it("shows the figure, its step and its hover text", () => {
    gauge.set(claude(130000, { compact_at_tokens: 200000 }), CATALOG);
    expect(gaugeNode().hidden).toBe(false);
    expect(gaugeNode().textContent).toBe("65%");
    expect(gaugeNode().dataset.step).toBe("accent");
    expect(gaugeNode().title).toBe("130k of 200k tokens (compacts at 200k), as of its last turn");
  });

  it("hides again when the record stops carrying a figure", () => {
    gauge.set(claude(612000), CATALOG);
    gauge.set(claude(null), CATALOG);
    expect(gaugeNode().hidden).toBe(true);
  });

  it("never touches the textarea when the figure changes", () => {
    const input = textarea();
    input.value = "half a sentence";
    input.focus();
    input.setSelectionRange(4, 4);
    gauge.set(claude(300000), CATALOG);
    gauge.set(claude(900000), CATALOG);
    expect(textarea()).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("half a sentence");
    expect(input.selectionStart).toBe(4);
    expect(gaugeNode().dataset.step).toBe("warning");
  });

  it("does not rewrite its own node for an unchanged figure", () => {
    gauge.set(claude(612000), CATALOG);
    const text = gaugeNode().firstChild;
    gauge.set(claude(612400), CATALOG);
    expect(gaugeNode().firstChild).toBe(text);
  });

  it("is a no-op on a composer without the slot", () => {
    document.body.innerHTML = "<div></div>";
    expect(mountContextGauge(document.body, { ids: IDS })).toBeNull();
  });
});
