import { describe, expect, it } from "vitest";
import { reviewCommentContext } from "../src/core/reviewCommentContext.js";
import { workspaceCommentMessages } from "../src/core/workspaceCommentMessages.js";

describe("review comment scope", () => {
  const paths = ["one.js", "two.js"];
  it("includes every file without a selection or ambient viewing context", () => {
    expect(reviewCommentContext({ paths, selected: new Set(), mode: "all" }).items).toEqual(
      paths.map((path) => ({ kind: "diff", path, mode: "all" })),
    );
  });
  it("uses selected files and preserves their unsaved selection", () => {
    const selection = { kind: "selection", path: "two.js", text: "draft", unsaved: true };
    expect(reviewCommentContext({ paths, selected: new Set(["two.js"]), mode: "uncommitted", snapshot: { items: [selection] } }).items).toEqual([
      selection, { kind: "diff", path: "two.js", mode: "uncommitted" },
    ]);
  });
  it("retains the exact historical commit alongside reviewed files", () => {
    const commit = "a".repeat(40);
    expect(reviewCommentContext({ paths, selected: new Set(["one.js"]), commit }).items).toEqual([
      { kind: "commit", sha: commit }, { kind: "file", path: "one.js" },
    ]);
  });
  it("ignores selections that belong to another changeset", () => {
    expect(reviewCommentContext({ paths, selected: new Set(["old.js"]), mode: "all" }).items).toHaveLength(2);
  });
});

describe("workspace comment paths", () => {
  const messages = [{ body: "fix", anchor: { path: "one.js" }, viewing_context: { version: 1, items: [{ kind: "diff", mode: "all", path: "two.js" }] } }];
  it("uses the actual source mount for anchors and selected files", () => {
    const result = workspaceCommentMessages(messages, { root: "/work", directories: [{ source_id: "s", name: "Pretty name", path: "/work/source-2" }] }, "s");
    expect(result[0].anchor.path).toBe("source-2/one.js");
    expect(result[0].viewing_context.items[0].path).toBe("source-2/two.js");
    expect(messages[0].anchor.path).toBe("one.js");
  });
  it("keeps adopted root-relative paths", () => {
    expect(workspaceCommentMessages(messages, { root: "/work", directories: [{ source_id: "s", path: "/work" }] }, "s")).toEqual(messages);
  });
  it("does not post with an unresolved source", () => {
    expect(() => workspaceCommentMessages(messages, { root: "/work", directories: [] }, "s")).toThrow("unavailable");
  });
});
