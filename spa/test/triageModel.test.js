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
