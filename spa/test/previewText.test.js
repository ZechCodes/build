import { describe, expect, it } from "vitest";
import { plainPreview } from "../src/core/previewText.js";
import { modelWord, overviewHtml, overviewRows, overviewState } from "../src/core/agentOverview.js";

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

  it("keeps the address of an autolink, and drops a tilde fence like a backtick one", () => {
    expect(plainPreview("See <https://example.test> and <zech@example.test>")).toBe("See https://example.test and zech@example.test");
    expect(plainPreview("See <https://example.test/a?b=1&c=2>.")).toBe("See https://example.test/a?b=1&c=2.");
    expect(plainPreview("Before\n\n~~~js\nconst x = 1;\n~~~\n\nafter")).toBe("Before after");
    expect(plainPreview("Before\n\n~~~\nconst x = 1;\n```\nnot closed by that\n~~~\nafter")).toBe("Before after");
    expect(plainPreview("~~gone~~ kept")).toBe("gone kept");
    expect(plainPreview("[Foo (bar)](https://w.test/Foo_(bar)) and ![shot (2)](a_(1).png) end")).toBe("Foo (bar) and shot (2) end");
    expect(plainPreview("an unclosed <a and a < b comparison and a 2 > 1 one")).toBe("an unclosed <a and a < b comparison and a 2 > 1 one");
  });

  it("takes no longer on a malformed 200 KB body than on a short one", () => {
    // Unclosed marks must not send a pattern scanning to the end of the body
    // from every opener: a preview is 240 characters, and the panel waits on it.
    const bodies = {
      tags: "<a ".repeat(70000),
      fence: `\`\`\`js\n${"const x = 1;\n".repeat(16000)}`,
      autolinks: "<https:".repeat(30000),
      links: "[a](".repeat(50000),
      emphasis: "**a __b ".repeat(25000),
    };
    for (const [name, body] of Object.entries(bodies)) {
      expect(body.length).toBeGreaterThanOrEqual(200000);
      const started = performance.now();
      const preview = plainPreview(body);
      const took = performance.now() - started;
      expect(took, `${name} took ${Math.round(took)} ms`).toBeLessThan(500);
      expect(preview.length).toBeLessThanOrEqual(240);
    }
    expect(plainPreview(bodies.fence)).toBe("");
  });

  it("never splits a surrogate pair, at the scan bound or at the cut", () => {
    const lone = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
    // 4095 units of tags put the emoji's two units across the 4096-unit scan bound.
    const atScanBound = plainPreview(`${"<b>".repeat(1365)}😀 is the answer`);
    expect(atScanBound).not.toMatch(lone);
    expect(atScanBound).toBe("");
    // 238 letters put the emoji across the 239-unit cut before the ellipsis.
    const atCut = plainPreview(`${"a".repeat(238)}😀 more`, 240);
    expect(atCut).not.toMatch(lone);
    expect(atCut).toBe(`${"a".repeat(238)}…`);
    // A pair that fits whole stays whole.
    expect(plainPreview(`${"a".repeat(237)}😀 more`, 240)).toBe(`${"a".repeat(237)}😀…`);
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
  it("ranks a failed start over a failed run over working over unread over starting over idle", () => {
    expect(overviewState({ start_error: "boom", working: true, unread_count: 1 }).state).toBe("error");
    expect(overviewState({ unread_count: 1, unread_reason: "run_failed" })).toMatchObject({ state: "error", word: "Failed" });
    expect(overviewState({ unread_count: 1, unread_reason: "run_failed", working: true })).toMatchObject({ state: "error", word: "Failed" });
    expect(overviewState({ unread_count: 1, unread_reason: "blocked", working: true })).toMatchObject({ state: "working", word: "Working" });
    expect(overviewState({ unread_count: 1, unread_reason: "blocked" })).toMatchObject({ state: "waiting", word: "Blocked" });
    expect(overviewState({ unread_count: 2 })).toMatchObject({ state: "waiting", word: "Unread", detail: "2 unread" });
    expect(overviewState({ working: true })).toMatchObject({ state: "working", word: "Working" });
    expect(overviewState({ state: "starting" })).toMatchObject({ state: "starting", word: "Starting" });
    expect(overviewState({ state: "live" })).toMatchObject({ state: "idle", word: "Idle" });
  });
});

describe("overviewHtml workspace summary", () => {
  const entry = (id, state) => ({ agent: { id, name: id, watched: true }, state: { ...state, id },
    source: { slot: "current" }, workspaceId: "ws-1", section: "workspace", sectionName: "Workspace one" });
  const summary = (rows) => overviewHtml(rows, { showProjectAgents: false, scope: { kind: "project" },
    workspaces: [{ workspaceId: "ws-1", name: "Workspace one" }], tasks: [] })
    .split('aria-label="Workspace one"')[1].split("</div>")[0];

  it("pulses while an agent works, even when that agent also has an unread message", () => {
    const rows = overviewRows([entry("busy", { working: true, unread_count: 1, unread_reason: "agent_message" })], [{ items: [] }]);
    expect(rows[0].state).toBe("working");
    expect(summary(rows)).toContain('class="rail-overview-live" title="1 working" role="img" aria-label="1 working"');
    expect(summary(rows)).toContain('title="1 unread">1<');
    expect(summary(rows)).not.toContain("rail-overview-idle");
  });

  // #192: the dot is a fixed slot, so it is drawn idle when nothing works and
  // the pill and the + hold their columns from heading to heading.
  it("draws the working dot idle, and says so, for a quiet workspace", () => {
    const rows = overviewRows([entry("quiet", { unread_count: 1, unread_reason: "agent_message" })], [{ items: [] }]);
    expect(summary(rows)).not.toContain("rail-overview-live");
    expect(summary(rows)).toContain('class="rail-overview-idle" title="Nothing working" role="img" aria-label="Nothing working"');
    expect(summary(rows).indexOf('title="1 unread"')).toBeLessThan(summary(rows).indexOf("rail-overview-idle"));
  });

  it("puts the idle dot after the No agents word on an empty workspace", () => {
    const html = summary(overviewRows([], []));
    expect(html.indexOf("rail-overview-none")).toBeLessThan(html.indexOf("rail-overview-idle"));
    expect(html).toContain("rail-overview-idle");
  });
});
