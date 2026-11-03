import { describe, expect, it } from "vitest";
import { reviewAnchorSelection, routeReviewSelection, selectionForReview, switchReviewSelection } from "../src/core/taskReviewSelection.js";

const commit = "a".repeat(40);
const directory = { id: "dir", is_git: true };
const review = { snapshots: [
  { id: "one", number: 1, directories: [directory] },
  { id: "two", number: 2, directories: [directory, { id: "notes", status: "not_git", is_git: false }] },
] };
const anchor = { snapshot_id: "one", directory_id: "dir", path: "src/a.js", side: "new", line: 42, commit };

describe("review URL selections", () => {
  it("restores a URL selection without reading any connection or local UI state", () => {
    const url = { revision: 1, mode: "commit", commit, directoryId: "dir", path: "src/a.js", side: "old", line: 3, view: "files" };
    const selection = selectionForReview(review, url);
    expect(selection).toMatchObject({ ...url, snapshotId: "one", anchor: { path: "src/a.js", side: "old", line: 3 } });
    expect(routeReviewSelection(selection)).toEqual(url);
  });

  it("chooses the latest revision and first directory for a plain task link", () => {
    expect(selectionForReview(review)).toMatchObject({ snapshotId: "two", revision: 2, directoryId: "dir", mode: "all", view: "changes", path: "" });
    expect(selectionForReview(null)).toBeNull();
    expect(selectionForReview(review, { revision: 2, directoryId: "notes" }).view).toBe("files");
  });

  it("keeps an explicitly selected revision when a newer revision arrives", () => {
    const selected = selectionForReview(review, { revision: 1, mode: "all", directoryId: "dir", path: "src/a.js", line: 8, side: "new" });
    const newer = { snapshots: [...review.snapshots, { id: "three", number: 3, directories: [directory] }] };
    expect(selectionForReview(newer, routeReviewSelection(selected))).toEqual(selected);
  });

  it("preserves a file and line on revision changes", () => {
    const selected = selectionForReview(review, { revision: 1, mode: "all", directoryId: "dir", path: "src/a.js", line: 8, side: "new" });
    const changed = switchReviewSelection(selected, { snapshotId: "two", revision: 2 });
    expect(changed).toMatchObject({ snapshotId: "two", revision: 2, path: "src/a.js", line: 8, anchor: { path: "src/a.js", side: "new", line: 8 } });
  });

  it("preserves the selected file across commits only if it appears in the known comparison", () => {
    const selected = selectionForReview(review, { revision: 1, mode: "all", path: "src/a.js", side: "new", line: 8 });
    const changes = { mode: "commit", commit };
    expect(switchReviewSelection(selected, changes, { files: [{ path: "src/a.js" }] })).toMatchObject({ path: "src/a.js", line: 8 });
    expect(switchReviewSelection(selected, changes, { files: [{ path: "other.js" }] })).toMatchObject({ path: "", line: null, side: null, anchor: null });
    expect(switchReviewSelection(selected, changes)).toMatchObject({ path: "src/a.js", line: 8 });
  });
});

describe("cached comment anchor routing", () => {
  it("opens the anchor's revision, commit, directory, path and line", () => {
    expect(reviewAnchorSelection(review, anchor, { support: { commit: true }, commits: { commits: [{ oid: commit }] } })).toMatchObject({
      snapshotId: "one", revision: 1, mode: "commit", commit, directoryId: "dir", path: "src/a.js", side: "new", line: 42,
    });
  });

  it("falls back to All changes when the commit is known unreadable or unsupported", () => {
    for (const options of [
      { support: { commit: false } },
      { support: { commit: true }, commits: { commits: [] } },
      { support: { commit: true }, commits: { read_error: "gone" } },
    ]) {
      expect(reviewAnchorSelection(review, anchor, options)).toMatchObject({ mode: "all", commit: null, path: "src/a.js", line: 42 });
    }
  });

  it("retains a tentative commit when its list is unknown or paginated", () => {
    for (const commits of [null, { commits: [], commits_truncated: true }, { commits: [], next_cursor: "next" }]) {
      expect(reviewAnchorSelection(review, anchor, { support: { commit: true }, commits })).toMatchObject({ mode: "commit", commit });
    }
  });

  it("routes old anchors to the range and rejects unavailable revision contexts", () => {
    const { commit: _commit, ...old } = anchor;
    expect(reviewAnchorSelection(review, old)).toMatchObject({ mode: "all", commit: null, line: 42 });
    expect(reviewAnchorSelection(review, { ...anchor, snapshot_id: "gone" })).toBeNull();
    expect(reviewAnchorSelection(review, { ...anchor, directory_id: "gone" })).toBeNull();
  });
});
