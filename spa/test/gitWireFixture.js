// The v2 git wire, in fixture form.
//
// git.status carries shape — which files changed, what each one holds
// (content_key) and one status_key over the lot — and git.diff carries one
// file's body at a time. A test that used to hand the pane a whole patch hands
// it a worktree instead: a map of path → the line that file holds now, from
// which both answers are derived, so an edit moves exactly the keys the bridge
// would move.

import { hashText } from "../src/core/reviewMemory.js";

/** One file's body: what it held (`old`) replaced by the line — or lines — it
 *  holds now. */
export function patchFor(path, line) {
  const added = Array.isArray(line) ? line : [line];
  const body = added.map((text) => `+${text}`).join("\n");
  return `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -1,2 +1,${added.length + 1} @@\n-old\n${body}\n`;
}

const contentKeyOf = (line) => `content-${hashText(JSON.stringify(line))}`;

const statusKeyOf = (shape) =>
  `shape-${hashText(
    JSON.stringify([
      shape.branch,
      shape.head,
      shape.repo_state,
      shape.upstream,
      shape.ahead,
      shape.behind,
      shape.stash_count,
      shape.files_truncated,
      shape.files,
    ]),
  )}`;

/** A worktree whose files are `{ path: line }`. `status()` answers the shape
 *  (its status_key moving with any of it), `diff({ paths })` answers those
 *  files' bodies, and `write`/`remove` move the tree under the pane. `base`
 *  fixes the fields every shape carries; `patchOf` is for a suite whose hunk
 *  ids are derived from a patch of its own shape. */
export function worktreeOf(lines = {}, { base = {}, patchOf = patchFor } = {}) {
  const contents = { ...lines };
  const fileOf = (path) => ({
    path,
    staged: "none",
    index_status: "M",
    worktree_status: "M",
    content_key: contentKeyOf(contents[path]),
    added: 1,
    deleted: 1,
    binary: false,
  });
  const status = (overrides = {}) => {
    const files = Object.keys(contents).map(fileOf);
    const shape = {
      branch: "main",
      path: "/repo",
      head: "f".repeat(40),
      repo_state: "clean",
      upstream: "origin/main",
      ahead: 0,
      behind: 0,
      stash_count: 0,
      files,
      files_truncated: false,
      stat: { files_changed: files.length, insertions: files.length, deletions: files.length },
      ...base,
      ...overrides,
    };
    return { ...shape, status_key: statusKeyOf(shape) };
  };
  const bodyOf = (path) => ({
    path,
    content_key: contentKeyOf(contents[path]),
    patch: contents[path] === undefined ? "" : patchOf(path, contents[path]),
    truncated: false,
  });
  return {
    status,
    diff: ({ paths = [] } = {}) => ({ files: paths.map(bodyOf) }),
    wholePatch: () => Object.keys(contents).map((path) => patchOf(path, contents[path])).join(""),
    write(path, line) {
      contents[path] = line;
    },
    remove(path) {
      delete contents[path];
    },
  };
}

/** The unchanged answer git.status gives when the key it was sent still stands. */
export const unchangedStatus = (status) => ({ unchanged: true, status_key: status.status_key });

/** The page a bridge that pages (`bodies.pages`, #95) cuts out of `whole`
 *  from `offset`: at most `pageBytes` bytes, ending on a line end unless one
 *  line is longer, and named by `version` — which a bridge takes from the
 *  whole text, never from the file's content key. The page's text is `patch`
 *  and where it sits is `range`, as `git.diff`'s file, `git.show` and
 *  `git.changeset_diff` answer it. */
export function pagedAnswer(whole, offset, { version = "v1", pageBytes = 262144 } = {}) {
  const bytes = new TextEncoder().encode(whole);
  let end = Math.min(bytes.length, offset + pageBytes);
  if (end < bytes.length) {
    const lineEnd = bytes.lastIndexOf(10, end - 1);
    if (lineEnd >= offset) end = lineEnd + 1;
  }
  return {
    patch: new TextDecoder().decode(bytes.subarray(offset, end)),
    range: { offset, end, total: bytes.length, version },
  };
}
