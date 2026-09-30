// @vitest-environment jsdom
// How full the agent's context is, beside the paperclip.
//
// The figure comes off the agent's digest (`last_context_tokens`, wire 1.10)
// against where the chat compacts (`compact_at_tokens`), or the window its
// model has when it never does. It takes no space when either is missing,
// and it is written onto a node of its own: the textarea beside it is never
// touched, so a push of the agent record mid-sentence keeps the caret.

import { describe, it, expect, beforeEach } from "vitest";
import { composerGaugeHtml, composerHtml, composerPartIds } from "../src/core/composer.js";
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
    expect(gauge).toMatchObject({ text: "190k", expanded: "190k/200k 95%", percent: 95, step: "warning" });
    expect(gauge.title).toBe("190k of 200k tokens (compacts at 200k), 95% used, as of its last turn");
  });

  it("measures against the model's window when the chat never compacts", () => {
    const gauge = contextGauge(claude(612000, { compact_at_tokens: 0 }), CATALOG);
    expect(gauge).toMatchObject({ text: "612k", expanded: "612k/1M 61%", step: "accent" });
    expect(gauge.title).toBe("612k of 1M window, 61% used, as of its last turn");
  });

  it("names a threshold the context has passed", () => {
    expect(contextGauge(claude(1200000), CATALOG).title).toBe("1.2M of 1M window, 120% used, as of its last turn");
    expect(contextGauge(claude(210000, { compact_at_tokens: 200000 }), CATALOG)).toMatchObject({
      text: "210k",
      expanded: "210k/200k 105%",
    });
  });

  it("reads the token total off the record at rest, the way a person reads it", () => {
    expect(contextGauge(claude(612000), CATALOG)).toMatchObject({ text: "612k", percent: 61 });
    expect(contextGauge(claude(52400), CATALOG).text).toBe("52k");
    expect(contextGauge(claude(1500000), CATALOG).text).toBe("1.5M");
  });

  it("expands to tokens, measure and percent", () => {
    expect(contextGauge(claude(52000, { compact_at_tokens: 200000 }), CATALOG).expanded).toBe("52k/200k 26%");
    expect(contextGauge(claude(52000), CATALOG).expanded).toBe("52k/1M 5%");
  });

  it("steps dim below 50%, accent from 50 to 80%, warning above", () => {
    expect(contextGauge(claude(490000), CATALOG).step).toBe("dim");
    expect(contextGauge(claude(500000), CATALOG).step).toBe("accent");
    expect(contextGauge(claude(800000), CATALOG).step).toBe("accent");
    expect(contextGauge(claude(810000), CATALOG).step).toBe("warning");
  });

  it("falls back to the window for a digest from before compaction", () => {
    expect(contextGauge(claude(612000), CATALOG).title).toBe("612k of 1M window, 61% used, as of its last turn");
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
    document.body.innerHTML = `<div class="rail-footer">${composerHtml({
      inputId: IDS.input,
      sendId: IDS.send,
      hintId: IDS.hint,
      placeholder: "Say",
      attachable: true,
      modelMenu: true,
    })}${composerGaugeHtml(IDS.input)}</div>`;
    gauge = mountContextGauge(document.body, { ids: IDS });
  });

  it("stands outside the box, over a room kept beside the paperclip, hidden until there is a figure", () => {
    // The file input between them is hidden and takes no space.
    expect(document.querySelector(".composer-actions > .composer-gauge-room + .composer-file + .composer-attach")).not.toBeNull();
    expect(document.querySelector(".composer").contains(gaugeNode())).toBe(false);
    expect(gaugeNode().hidden).toBe(true);
  });

  it("shows the token total, its step and its hover text", () => {
    gauge.set(claude(130000, { compact_at_tokens: 200000 }), CATALOG);
    expect(gaugeNode().hidden).toBe(false);
    expect(gaugeNode().textContent).toBe("130k");
    expect(gaugeNode().dataset.step).toBe("accent");
    expect(gaugeNode().title).toBe("130k of 200k tokens (compacts at 200k), 65% used, as of its last turn");
    expect(gaugeNode().getAttribute("aria-label")).toBe(gaugeNode().title);
  });

  it("carries the rest of the expanded figure for the stylesheet to add after the total", () => {
    // Hover and focus append "/<measure> <percent>" after the total, so the
    // gauge reads "130k/200k 65%" and grows leftward from its right edge.
    gauge.set(claude(130000, { compact_at_tokens: 200000 }), CATALOG);
    expect(gaugeNode().dataset.measure).toBe("200k");
    expect(gaugeNode().dataset.percent).toBe("65%");
    expect(`${gaugeNode().textContent}/${gaugeNode().dataset.measure} ${gaugeNode().dataset.percent}`).toBe(
      contextGauge(claude(130000, { compact_at_tokens: 200000 }), CATALOG).expanded,
    );
  });

  it("is reachable from the keyboard, so the expanded figure is too", () => {
    expect(gaugeNode().tabIndex).toBe(0);
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

  it("does not rewrite its own node for an unchanged figure", async () => {
    gauge.set(claude(612000), CATALOG);
    const text = gaugeNode().firstChild;
    const writes = [];
    new MutationObserver((records) => writes.push(...records)).observe(gaugeNode(), {
      attributes: true,
      characterData: true,
      childList: true,
      subtree: true,
    });
    gauge.set(claude(612400), CATALOG);
    await Promise.resolve();
    expect(gaugeNode().firstChild).toBe(text);
    expect(writes).toEqual([]);
  });

  it("is a no-op on a composer without the slot", () => {
    document.body.innerHTML = "<div></div>";
    expect(mountContextGauge(document.body, { ids: IDS })).toBeNull();
  });
});
