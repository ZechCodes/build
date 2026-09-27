import { describe, expect, it } from "vitest";
import { plainPreview } from "../src/core/previewText.js";
import { modelWord, overviewState } from "../src/core/agentOverview.js";

describe("plainPreview (#186)", () => {
  it("turns a markdown table into its words, rule row and pipes gone", () => {
    expect(plainPreview("| Issue | Verdict | Evidence / remaining work |\n|---|:---:|---|\n| #183 | **Done** | fixed |"))
      .toBe("Issue Verdict Evidence / remaining work #183 Done fixed");
  });

  it("takes headings, emphasis, code, links, quotes and list marks off and keeps the words", () => {
    expect(plainPreview("## Report\n\n- **Bold** and _em_ and `code`\n> quoted\n1. [a link](https://x.test) ![alt](i.png)\n---\nsnake_case_name stays"))
      .toBe("Report Bold and em and code quoted a link alt snake_case_name stays");
  });

  it("drops fenced code and html, and folds whitespace onto one line", () => {
    expect(plainPreview("Before\n\n```js\nconst x = 1;\n```\n\n<b>after</b>   \t the   fence\n")).toBe("Before after the fence");
    expect(plainPreview("")).toBe("");
    expect(plainPreview(null)).toBe("");
  });

  it("cuts a long body with an ellipsis", () => {
    const long = "word ".repeat(100).trim();
    const cut = plainPreview(long, 40);
    expect(cut.length).toBeLessThanOrEqual(40);
    expect(cut.endsWith("…")).toBe(true);
  });
});

describe("modelWord", () => {
  it("says the model the way a row can wear it", () => {
    expect(modelWord({ active_model: "claude-opus-5" })).toBe("Opus 5");
    expect(modelWord({ model: "claude-fable-5-1" })).toBe("Fable 5.1");
    expect(modelWord({ active_model: "claude-haiku-4-5-20251001" })).toBe("Haiku 4.5");
    expect(modelWord({ active_model: "gpt-6-astra" })).toBe("GPT 6 Astra");
    expect(modelWord({ active_model: "", model: "", provider: "codex" })).toBe("Codex TUI");
    expect(modelWord({ provider: "claude_adk" })).toBe("Claude Code");
  });
});

describe("overviewState", () => {
  it("ranks a failed start over unread over working over starting over idle", () => {
    expect(overviewState({ start_error: "boom", working: true, unread_count: 1 }).state).toBe("error");
    expect(overviewState({ unread_count: 1, unread_reason: "run_failed" })).toMatchObject({ state: "error", word: "Failed" });
    expect(overviewState({ unread_count: 1, unread_reason: "blocked", working: true })).toMatchObject({ state: "waiting", word: "Blocked" });
    expect(overviewState({ unread_count: 2 })).toMatchObject({ state: "waiting", word: "Unread", detail: "2 unread" });
    expect(overviewState({ working: true })).toMatchObject({ state: "working", word: "Working" });
    expect(overviewState({ state: "starting" })).toMatchObject({ state: "starting", word: "Starting" });
    expect(overviewState({ state: "live" })).toMatchObject({ state: "idle", word: "Idle" });
  });
});
