// The triage overlay as markup: the banner over the stack, the surfaced
// criticals with their level chips, the named collapsed groups, and the plain
// stack the dial (or a missing pass) falls back to. The generated-files group
// stays what it always was — its own group, at the very bottom.

import { describe, it, expect } from "vitest";
import { parseDiff, patchHunks } from "../src/core/diff.js";
import { diffStackEntries, diffStackHtml } from "../src/core/diffRender.js";

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

  it("puts the critical file first, under a line that says why it is there", () => {
    const html = stack({ triage: triage() });
    expect(html).toContain('class="tsectionhead"');
    expect(html).toContain('class="file capped tcritical"');
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
    expect(html).toContain('class="tgrouphead open"');
    expect(html).toContain('aria-expanded="true"');
    expect(html).toContain('class="file capped tgrouped"');
  });

  it("says the pass is stale when the diff moved under it, and still orders by it", () => {
    const html = stack({ triage: triage({ stale: true }) });
    expect(html).toContain("triage from an earlier revision — re-triaging");
    expect(html).toContain('class="file capped tcritical"');
  });

  it("labels an untriaged changeset instead of pretending it was ordered", () => {
    const html = stack({ triage: null });
    expect(html).toContain('class="tuntriaged"');
    expect(html).toContain("untriaged");
    expect(html).not.toContain("tcritical");
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
    expect(html).not.toContain("tcritical");
    expect(html).not.toContain("tgroup");
    expect(html).toContain('aria-pressed="true"');
    expect(html).toContain("Show ordered diff");
    expect(html.indexOf("src/crypto.rs")).toBeLessThan(html.indexOf("Cargo.toml"));
  });

  it("carries the re-review chips and per-file controls through the ordering", () => {
    const html = diffStackHtml(FILES, {
      review: { patch: PATCH, triage: triage() },
      changedSince: new Set(["src/crypto.rs", "Cargo.toml"]),
      commentable: true,
    });
    // The surfaced critical and the file folded into a group both keep them.
    expect((html.match(/changed since your review/g) || []).length).toBe(2);
    // Both readable files keep their ✎; the noise group is still collapsed.
    expect((html.match(/class="fcmt"/g) || []).length).toBe(2);
  });

  it("leaves the generated-files group its own group at the very bottom", () => {
    const html = stack({ triage: triage() });
    expect(html).toContain("noisegroup");
    expect(html.lastIndexOf("noisegroup")).toBeGreaterThan(html.lastIndexOf("tgroup"));
    expect(html).not.toContain('data-file="uv.lock"'); // collapsed, as before
  });
});

// The ordered stack is a keyed list like any other: one entry per block a
// repaint can move. A section is a head and then its files, each under its own
// name, so a file arriving above the one being read moves neither the reader
// nor the element they are holding.
describe("the ordered stack as keyed entries", () => {
  const entries = (review) => diffStackEntries(FILES, { review: { patch: PATCH, ...review } });

  it("names the bar, the section head, and every file under its own key", () => {
    expect(entries({ triage: triage() }).map((entry) => entry.key)).toEqual([
      "triagebar",
      "sectionhead:critical",
      "EDIT:src/crypto.rs",
      "group:Version bumps",
      "noise",
    ]);
  });

  it("opens a group into a head and the files under it", () => {
    expect(entries({ triage: triage(), expandedGroups: new Set(["Version bumps"]) }).map((entry) => entry.key)).toEqual([
      "triagebar",
      "sectionhead:critical",
      "EDIT:src/crypto.rs",
      "grouphead:Version bumps",
      "EDIT:Cargo.toml",
      "noise",
    ]);
  });

  it("names every file of an untriaged stack, under the bar", () => {
    expect(entries({ triage: null }).map((entry) => entry.key)).toEqual([
      "triagebar",
      "EDIT:src/crypto.rs",
      "EDIT:Cargo.toml",
      "noise",
    ]);
  });

  it("says which section a file belongs to on the file itself", () => {
    const critical = entries({ triage: triage() }).find((entry) => entry.key === "EDIT:src/crypto.rs");
    expect(critical.html).toContain("tcritical");
  });

  it("is what diffStackHtml is made of, overlay and all", () => {
    for (const review of [{ triage: triage() }, { triage: null }, { triage: triage(), dial: true }]) {
      expect(entries(review).map((entry) => entry.html).join("")).toBe(diffStackHtml(FILES, { review: { patch: PATCH, ...review } }));
    }
  });
});

// Every triage decision is overridable (the issue doc's fourth principle), and
// the offer sits on the hunk the decision was about.
describe("the override controls on an ordered stack", () => {
  const overridable = (review) => stack({ overridable: true, ...review });

  const controlsIn = (html, path) => {
    const file = html.slice(html.indexOf(`data-file="${path}"`));
    return file.slice(0, file.indexOf("</table>"));
  };

  it("offers to collapse a surfaced critical and to keep a collapsed hunk surfaced", () => {
    const html = overridable({ triage: triage() });
    const critical = controlsIn(html, "src/crypto.rs");
    expect(critical).toContain(`data-direction="collapse"`);
    expect(critical).toContain(`data-hunk="${ids["src/crypto.rs"]}"`);
    expect(critical).toContain("Collapse");
    const collapsed = controlsIn(html, "Cargo.toml");
    expect(collapsed).toContain(`data-direction="surface"`);
    expect(collapsed).toContain("Keep surfaced");
  });

  it("offers nothing on a hunk the pass never named", () => {
    const html = overridable({ triage: triage({ hunks: [{ hunk_id: ids["src/crypto.rs"], level: "critical" }] }) });
    expect(controlsIn(html, "Cargo.toml")).not.toContain("toverride");
  });

  it("draws no controls where the surface cannot post one", () => {
    expect(stack({ triage: triage() })).not.toContain("toverride");
  });

  it("says on the hunk when the reading there is the reviewer's own, and what they said", () => {
    const html = overridable({
      triage: triage({
        overrides: [{ hunk_id: ids["Cargo.toml"], direction: "surface", note: "this bump shipped the outage" }],
      }),
    });
    const overridden = controlsIn(html, "Cargo.toml");
    expect(overridden).toContain('class="hchip overridden"');
    expect(overridden).toContain("your call: surfaced");
    expect(overridden).toContain("this bump shipped the outage");
    // And the way back is the control it now offers.
    expect(overridden).toContain(`data-direction="collapse"`);
  });

  it("escapes what the reviewer wrote", () => {
    const html = overridable({
      triage: triage({
        overrides: [{ hunk_id: ids["Cargo.toml"], direction: "surface", note: '<img src=x onerror="boom">' }],
      }),
    });
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
  });
});
