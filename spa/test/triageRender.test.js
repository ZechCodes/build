// The triage overlay as markup: the banner over the stack, the surfaced
// criticals with their level chips, the named collapsed groups, and the plain
// stack the dial (or a missing pass) falls back to. The generated-files group
// stays what it always was — its own group, at the very bottom.

import { describe, it, expect } from "vitest";
import { parseDiff, patchHunks } from "../src/core/diff.js";
import { diffStackHtml } from "../src/core/diffRender.js";

const patchFor = (path, line) =>
  `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -1,2 +1,2 @@\n-old\n+${line}\n context\n`;

const PATCH = patchFor("src/crypto.rs", "seal(key)") + patchFor("Cargo.toml", 'v = "2"') + patchFor("uv.lock", "locked");
const FILES = parseDiff(PATCH);
const ids = Object.fromEntries(patchHunks(PATCH).map((hunk) => [hunk.path, hunk.hunk_id]));

const triage = (overrides = {}) => ({
  based_on: "rev-1",
  stale: false,
  overrides: [],
  hunks: [
    { hunk_id: ids["src/crypto.rs"], level: "critical", rationale: "changes how the key is sealed" },
    { hunk_id: ids["Cargo.toml"], level: "low", group: "Version bumps", rationale: "a version string" },
  ],
  ...overrides,
});

const stack = (review) => diffStackHtml(FILES, { review: { patch: PATCH, ...review } });

describe("diffStackHtml with a triage overlay", () => {
  it("is byte-identical to the plain stack when no surface plugs the overlay in", () => {
    expect(diffStackHtml(FILES)).toBe(diffStackHtml(FILES, {}));
    expect(diffStackHtml(FILES)).not.toContain("triagebar");
  });

  it("puts the critical file first, under a section that says why it is there", () => {
    const html = stack({ triage: triage() });
    expect(html).toContain('class="tsection tcritical"');
    expect(html).toContain("Needs review first");
    expect(html.indexOf("src/crypto.rs")).toBeLessThan(html.indexOf("Cargo.toml"));
  });

  it("chips a surfaced critical hunk with its level and its rationale", () => {
    const html = stack({ triage: triage() });
    expect(html).toContain('class="hchip critical"');
    expect(html).toContain('title="changes how the key is sealed"');
    expect(html).toContain(`data-hunk="${ids["src/crypto.rs"]}"`);
  });

  it("collapses the low file into its named group, with a rationale and counts", () => {
    const html = stack({ triage: triage() });
    expect(html).toContain('class="tgroup"');
    expect(html).toContain("Version bumps");
    expect(html).toContain("1 file · 1 hunk");
    expect(html).toContain("a version string");
    expect(html).toContain('aria-expanded="false"');
  });

  it("keeps a collapsed group's diffs in the DOM — collapsed, never dropped", () => {
    const html = stack({ triage: triage() });
    const group = html.slice(html.indexOf('class="tgroup"'));
    expect(group).toContain('data-file="Cargo.toml"');
  });

  it("opens the group the reviewer expanded", () => {
    const html = stack({ triage: triage(), expandedGroups: new Set(["Version bumps"]) });
    expect(html).toContain('class="tgroup open"');
    expect(html).toContain('aria-expanded="true"');
  });

  it("says the pass is stale when the diff moved under it, and still orders by it", () => {
    const html = stack({ triage: triage({ stale: true }) });
    expect(html).toContain("triage from an earlier revision — re-triaging");
    expect(html).toContain('class="tsection tcritical"');
  });

  it("labels an untriaged changeset instead of pretending it was ordered", () => {
    const html = stack({ triage: null });
    expect(html).toContain('class="tuntriaged"');
    expect(html).toContain("untriaged");
    expect(html).not.toContain("tsection tcritical");
    // Every file is still there, in patch order.
    expect(html.indexOf("src/crypto.rs")).toBeLessThan(html.indexOf("Cargo.toml"));
  });

  it("marks the hunks a pass never named, without covering the stack in chips", () => {
    const html = stack({ triage: triage({ hunks: [{ hunk_id: ids["src/crypto.rs"], level: "critical", rationale: "the seal" }] }) });
    expect(html).toContain('class="hchip untriaged"');
    expect((html.match(/hchip untriaged/g) || []).length).toBe(1);
  });

  it("offers the dial only where there is a pass to trust", () => {
    expect(stack({ triage: triage() })).toContain('class="tdial"');
    expect(stack({ triage: null })).not.toContain('class="tdial"');
  });

  it("renders the untriaged full stack when the dial is turned off the overlay", () => {
    const html = stack({ triage: triage(), dial: true });
    expect(html).not.toContain("tsection tcritical");
    expect(html).not.toContain("tgroup");
    expect(html).toContain('aria-pressed="true"');
    expect(html).toContain("Show ordered diff");
    expect(html.indexOf("src/crypto.rs")).toBeLessThan(html.indexOf("Cargo.toml"));
  });

  it("leaves the generated-files group its own group at the very bottom", () => {
    const html = stack({ triage: triage() });
    expect(html).toContain("noisegroup");
    expect(html.lastIndexOf("noisegroup")).toBeGreaterThan(html.lastIndexOf("tgroup"));
    expect(html).not.toContain('data-file="uv.lock"'); // collapsed, as before
  });
});
