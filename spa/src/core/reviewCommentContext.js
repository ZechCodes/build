import { normalizeViewingContext } from "./viewingContext.js";

/** A note reviews the selected files, or the whole changeset when none are
 * selected. Viewport visibility must never silently narrow that scope. */
export function reviewCommentContext({ paths, selected, mode, snapshot, commit = null }) {
  const selectedPaths = paths.filter((path) => selected.has(path));
  const reviewedPaths = selectedPaths.length ? selectedPaths : paths;
  const files = reviewedPaths.map((path) => mode ? { kind: "diff", path, mode } : { kind: "file", path });
  const selections = (snapshot?.items || []).filter((item) => item.kind === "selection" && reviewedPaths.includes(item.path));
  return normalizeViewingContext([...selections, ...(commit ? [{ kind: "commit", sha: commit }] : []), ...files]);
}
