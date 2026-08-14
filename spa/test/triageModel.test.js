// The review-prioritization overlay's decisions, as pure functions: how a
// triage report joins onto the diff the reviewer is looking at, what order the
// stack comes out in, and what the surface says when the report is missing,
// stale, or about hunks this changeset does not contain.

import { describe, it, expect } from "vitest";
import { parseDiff, patchHunks } from "../src/core/diff.js";
import {
  planChangesetTriage,
  loadTrustDial,
  saveTrustDial,
  triageSummaryLine,
  triageFingerprint,
  applyTriageOverride,
  applyTriageOverrides,
  overrideDirectionFor,
  unsettledOverrides,
} from "../src/core/triageModel.js";

/** A one-hunk patch for `path` whose changed line is `line`. */
const patchFor = (path, line) =>
  `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -1,2 +1,2 @@\n-old\n+${line}\n context\n`;

const PATCH = patchFor("src/crypto.rs", "seal(key)") + patchFor("Cargo.toml", 'version = "0.2.0"') + patchFor("docs/readme.md", "a word");

const idsOf = (patch) => Object.fromEntries(patchHunks(patch).map((hunk) => [hunk.path, hunk.hunk_id]));
const ids = idsOf(PATCH);

const plan = (triage, patch = PATCH) =>
  planChangesetTriage({ files: parseDiff(patch), patch, triage });

const triageOf = (hunks, extra = {}) => ({ based_on: "rev-1", hunks, overrides: [], stale: false, ...extra });

const pathsIn = (section) => section.files.map((file) => file.path);

describe("planChangesetTriage", () => {
  it("orders critical files ahead of the rest, in patch order within each section", () => {
    const result = plan(
      triageOf([
        { hunk_id: ids["src/crypto.rs"], level: "critical", rationale: "changes how the key is sealed" },
        { hunk_id: ids["Cargo.toml"], level: "normal" },
        { hunk_id: ids["docs/readme.md"], level: "normal" },
      ]),
    );
    expect(result.status).toBe("ordered");
    expect(result.sections.map((section) => section.kind)).toEqual(["critical", "normal"]);
    expect(pathsIn(result.sections[0])).toEqual(["src/crypto.rs"]);
    expect(pathsIn(result.sections[1])).toEqual(["Cargo.toml", "docs/readme.md"]);
  });

  it("marks the critical hunks so the render can chip them, rationale and all", () => {
    const result = plan(
      triageOf([{ hunk_id: ids["src/crypto.rs"], level: "critical", rationale: "changes how the key is sealed" }]),
    );
    const [critical] = result.sections[0].files;
    expect(critical.triageHunks).toEqual([
      {
        hunk_id: ids["src/crypto.rs"],
        level: "critical",
        rationale: "changes how the key is sealed",
        untriaged: false,
        overridden: false,
        overrideDirection: "",
        note: "",
        group: "",
      },
    ]);
  });

  it("collapses low hunks into their named group, with one rationale and its counts", () => {
    const result = plan(
      triageOf([
        { hunk_id: ids["src/crypto.rs"], level: "critical", rationale: "the seal" },
        { hunk_id: ids["Cargo.toml"], level: "low", group: "Version bumps", rationale: "a version string" },
        { hunk_id: ids["docs/readme.md"], level: "low", group: "Version bumps" },
      ]),
    );
    const group = result.sections.find((section) => section.kind === "group");
    expect(group.name).toBe("Version bumps");
    expect(group.rationale).toBe("a version string");
    expect(group.fileCount).toBe(2);
    expect(group.hunkCount).toBe(2);
    expect(pathsIn(group)).toEqual(["Cargo.toml", "docs/readme.md"]);
    // Groups sit under the surfaced work, never above it.
    expect(result.sections.map((section) => section.kind)).toEqual(["critical", "group"]);
  });

  it("keeps a low hunk in place when its file also carries a critical one", () => {
    const twoHunkPatch =
      "diff --git a/src/crypto.rs b/src/crypto.rs\n" +
      "index 1111111..2222222 100644\n--- a/src/crypto.rs\n+++ b/src/crypto.rs\n" +
      "@@ -1,2 +1,2 @@\n-old\n+seal(key)\n context\n" +
      "@@ -40,2 +40,2 @@\n-// note\n+// nit\n tail\n";
    const hunks = patchHunks(twoHunkPatch);
    const result = plan(
      triageOf([
        { hunk_id: hunks[0].hunk_id, level: "critical", rationale: "the seal" },
        { hunk_id: hunks[1].hunk_id, level: "low", group: "Comment tidy" },
      ]),
      twoHunkPatch,
    );
    expect(result.sections.map((section) => section.kind)).toEqual(["critical"]);
    const [file] = result.sections[0].files;
    expect(file.triageHunks.map((hunk) => hunk.level)).toEqual(["critical", "low"]);
  });

  it("renders a hunk the report does not name as normal, marked untriaged", () => {
    const result = plan(triageOf([{ hunk_id: ids["src/crypto.rs"], level: "critical", rationale: "the seal" }]));
    const normal = result.sections.find((section) => section.kind === "normal");
    expect(pathsIn(normal)).toEqual(["Cargo.toml", "docs/readme.md"]);
    expect(normal.files.every((file) => file.triageHunks.every((hunk) => hunk.untriaged))).toBe(true);
    expect(result.counts.untriaged).toBe(2);
  });

  it("says stale when the diff moved under the pass, and still orders by it", () => {
    const result = plan(
      triageOf([{ hunk_id: ids["src/crypto.rs"], level: "critical", rationale: "the seal" }], { stale: true }),
    );
    expect(result.status).toBe("stale");
    expect(result.sections[0].kind).toBe("critical");
  });

  it("falls back to the plain stack when there is no triage at all", () => {
    const result = plan(null);
    expect(result.status).toBe("none");
    expect(result.sections.map((section) => section.kind)).toEqual(["normal"]);
    expect(pathsIn(result.sections[0])).toEqual(["src/crypto.rs", "Cargo.toml", "docs/readme.md"]);
    // Nothing was classified, so nothing is marked: a plain stack, not a stack
    // covered in "untriaged" chips.
    expect(result.sections[0].files.every((file) => file.triageHunks === undefined)).toBe(true);
  });

  it("falls back to the plain stack when the pass is about another changeset entirely", () => {
    const result = plan(triageOf([{ hunk_id: "hdeadbeefdead", level: "critical", rationale: "elsewhere" }]));
    expect(result.status).toBe("none");
    expect(result.sections.map((section) => section.kind)).toEqual(["normal"]);
  });

  it("takes the reviewer's override over the pass's own level", () => {
    const result = plan(
      triageOf(
        [
          { hunk_id: ids["src/crypto.rs"], level: "critical", rationale: "the seal" },
          { hunk_id: ids["Cargo.toml"], level: "low", group: "Version bumps" },
        ],
        {
          overrides: [
            { hunk_id: ids["Cargo.toml"], direction: "surface", at: "2026-08-13T00:00:00Z" },
            { hunk_id: ids["src/crypto.rs"], direction: "collapse", at: "2026-08-13T00:00:00Z" },
          ],
        },
      ),
    );
    const normal = result.sections.find((section) => section.kind === "normal");
    expect(pathsIn(normal)).toContain("Cargo.toml");
    expect(normal.files[0].triageHunks[0].overridden).toBe(true);
    const group = result.sections.find((section) => section.kind === "group");
    expect(pathsIn(group)).toContain("src/crypto.rs");
  });

  it("counts what the banner says", () => {
    const result = plan(
      triageOf([
        { hunk_id: ids["src/crypto.rs"], level: "critical", rationale: "the seal" },
        { hunk_id: ids["Cargo.toml"], level: "low", group: "Version bumps" },
        { hunk_id: ids["docs/readme.md"], level: "normal" },
      ]),
    );
    expect(result.counts).toEqual({ critical: 1, normal: 1, low: 1, untriaged: 0 });
    expect(triageSummaryLine(result)).toBe("1 hunk needs review first · 1 collapsed");
  });

  it("holds still on an empty changeset", () => {
    const result = planChangesetTriage({ files: [], patch: "", triage: null });
    expect(result.status).toBe("none");
    expect(result.sections[0].files).toEqual([]);
  });
});

// The reviewer's half of the overlay. A disagreement is applied to the plan the
// moment it is made — before the bridge has answered — so the same pure join
// that renders the pass's reading renders the reviewer's correction of it, and
// the two are told apart on the hunk.
describe("overriding what the pass decided", () => {
  const PASS = triageOf([
    { hunk_id: ids["src/crypto.rs"], level: "critical", rationale: "the seal" },
    { hunk_id: ids["Cargo.toml"], level: "low", group: "Version bumps", rationale: "a version string" },
  ]);

  const markFor = (result, path) =>
    result.sections.flatMap((section) => section.files).find((file) => file.path === path).triageHunks[0];

  it("names the disagreement each hunk offers, and none where there is nothing to disagree with", () => {
    const result = plan(PASS);
    expect(overrideDirectionFor(markFor(result, "src/crypto.rs"))).toBe("collapse");
    expect(overrideDirectionFor(markFor(result, "Cargo.toml"))).toBe("surface");
    // A hunk the pass never named: there is no decision here to disagree with.
    expect(overrideDirectionFor(markFor(result, "docs/readme.md"))).toBe(null);
  });

  it("offers the way back on a hunk the reviewer already moved", () => {
    const surfaced = applyTriageOverride(PASS, { hunk_id: ids["Cargo.toml"], direction: "surface" });
    expect(overrideDirectionFor(markFor(plan(surfaced), "Cargo.toml"))).toBe("collapse");
    const collapsed = applyTriageOverride(PASS, { hunk_id: ids["src/crypto.rs"], direction: "collapse" });
    expect(overrideDirectionFor(markFor(plan(collapsed), "src/crypto.rs"))).toBe("surface");
  });

  it("moves a collapsed hunk into the stack, leaving the pass's own reading untouched", () => {
    const overridden = applyTriageOverride(PASS, {
      hunk_id: ids["Cargo.toml"],
      direction: "surface",
      note: "a version bump is how the last outage shipped",
    });
    expect(PASS.overrides).toEqual([]); // pure: the pass it was given is unchanged
    const result = plan(overridden);
    const normal = result.sections.find((section) => section.kind === "normal");
    expect(pathsIn(normal)).toContain("Cargo.toml");
    const mark = markFor(result, "Cargo.toml");
    expect(mark.overridden).toBe(true);
    expect(mark.overrideDirection).toBe("surface");
    expect(mark.note).toBe("a version bump is how the last outage shipped");
  });

  it("folds a surfaced hunk into a group of the reviewer's own, not the pass's", () => {
    const result = plan(applyTriageOverride(PASS, { hunk_id: ids["src/crypto.rs"], direction: "collapse" }));
    expect(result.sections.find((section) => section.kind === "critical")).toBeUndefined();
    const group = result.sections.find((section) => section.name === "Collapsed by you");
    expect(pathsIn(group)).toEqual(["src/crypto.rs"]);
    expect(markFor(result, "src/crypto.rs").overrideDirection).toBe("collapse");
  });

  it("gives a group the reviewer filled the reviewer's own reason, never the pass's", () => {
    const noted = plan(
      applyTriageOverride(PASS, {
        hunk_id: ids["src/crypto.rs"],
        direction: "collapse",
        note: "a rename, nothing more",
      }),
    );
    expect(noted.sections.find((section) => section.kind === "group").rationale).toBe("a rename, nothing more");
    // And with nothing said, the header says nothing — the pass's line for a
    // hunk it called critical is not a reason for collapsing it.
    const silent = plan(applyTriageOverride(PASS, { hunk_id: ids["src/crypto.rs"], direction: "collapse" }));
    expect(silent.sections.find((section) => section.kind === "group").rationale).toBe("");
  });

  it("keeps one word per hunk: saying it again replaces what was said before", () => {
    const once = applyTriageOverride(PASS, { hunk_id: ids["Cargo.toml"], direction: "surface", note: "first" });
    const twice = applyTriageOverride(once, { hunk_id: ids["Cargo.toml"], direction: "collapse", note: "second" });
    expect(twice.overrides).toEqual([{ hunk_id: ids["Cargo.toml"], direction: "collapse", note: "second" }]);
  });

  it("refuses a direction it cannot render rather than dropping the hunk somewhere", () => {
    expect(() => applyTriageOverride(PASS, { hunk_id: ids["Cargo.toml"], direction: "delete" })).toThrow(/surface/);
    expect(() => applyTriageOverride(PASS, { direction: "surface" })).toThrow(/hunk/);
    expect(() => applyTriageOverride(null, { hunk_id: "h1", direction: "surface" })).toThrow(/pass/);
  });

  it("applies a whole queue of them in order", () => {
    const result = applyTriageOverrides(PASS, [
      { hunk_id: ids["Cargo.toml"], direction: "surface" },
      { hunk_id: ids["src/crypto.rs"], direction: "collapse" },
    ]);
    expect(result.overrides.map((override) => override.direction)).toEqual(["surface", "collapse"]);
  });

  // What a client holds while the bridge catches up: an override it has sent
  // stays applied locally until the pass comes back carrying it, and is dropped
  // the moment it does — so the reviewer never sees their own decision flicker.
  it("holds a sent override only until the pass comes back carrying it", () => {
    const pending = [
      { hunk_id: ids["Cargo.toml"], direction: "surface" },
      { hunk_id: ids["src/crypto.rs"], direction: "collapse" },
    ];
    expect(unsettledOverrides(PASS, pending)).toEqual(pending);
    const answered = applyTriageOverride(PASS, { hunk_id: ids["Cargo.toml"], direction: "surface", note: "" });
    expect(unsettledOverrides(answered, pending)).toEqual([pending[1]]);
    // An answer that says something else about the hunk is not this one.
    const otherWay = applyTriageOverride(PASS, { hunk_id: ids["Cargo.toml"], direction: "collapse" });
    expect(unsettledOverrides(otherWay, pending)).toEqual(pending);
  });
});

describe("triageFingerprint", () => {
  const hunks = [{ hunk_id: "h1", level: "critical", rationale: "the seal" }];

  it("says nothing changed while the pass has not", () => {
    expect(triageFingerprint(triageOf(hunks))).toBe(triageFingerprint(triageOf(hunks)));
  });

  it("moves when a re-pass over the same revision reclassifies a hunk", () => {
    expect(triageFingerprint(triageOf(hunks))).not.toBe(
      triageFingerprint(triageOf([{ hunk_id: "h1", level: "low", group: "Nits" }])),
    );
  });

  it("moves when the diff goes stale under the pass, or the reviewer disagrees", () => {
    expect(triageFingerprint(triageOf(hunks))).not.toBe(triageFingerprint(triageOf(hunks, { stale: true })));
    expect(triageFingerprint(triageOf(hunks))).not.toBe(
      triageFingerprint(triageOf(hunks, { overrides: [{ hunk_id: "h1", direction: "collapse", at: "now" }] })),
    );
  });

  it("moves when the reviewer's note lands on a disagreement already made", () => {
    const bare = triageOf(hunks, { overrides: [{ hunk_id: "h1", direction: "collapse" }] });
    const noted = triageOf(hunks, { overrides: [{ hunk_id: "h1", direction: "collapse", note: "it is a rename" }] });
    expect(triageFingerprint(bare)).not.toBe(triageFingerprint(noted));
  });

  it("tells a missing pass apart from an empty one", () => {
    expect(triageFingerprint(null)).not.toBe(triageFingerprint(triageOf([])));
  });
});

describe("the trust dial", () => {
  const storage = () => {
    const values = new Map();
    return {
      getItem: (key) => (values.has(key) ? values.get(key) : null),
      setItem: (key, value) => values.set(key, String(value)),
      removeItem: (key) => values.delete(key),
    };
  };

  it("defaults to the ordered stack — the overlay is on until the reviewer turns it off", () => {
    expect(loadTrustDial("proj-1", storage())).toBe(false);
  });

  it("remembers the reviewer's choice per project", () => {
    const store = storage();
    saveTrustDial("proj-1", true, store);
    expect(loadTrustDial("proj-1", store)).toBe(true);
    expect(loadTrustDial("proj-2", store)).toBe(false);
    saveTrustDial("proj-1", false, store);
    expect(loadTrustDial("proj-1", store)).toBe(false);
  });

  it("keeps no preference for a surface with no project to key it by", () => {
    const store = storage();
    saveTrustDial(null, true, store);
    expect(loadTrustDial(null, store)).toBe(false);
  });
});
