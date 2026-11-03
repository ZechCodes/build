// Review commit reads and cache keys use full OIDs. A short reference must be
// unique within the selected saved directory's complete commit list first.
export const hasReviewCommitPrefix = (selection) => selection?.mode === "commit"
  && /^[0-9a-f]{4,39}$/i.test(selection.commit || "");
const oidOf = (row) => row.oid || row.sha || row.hash || row.commit || "";
const completeList = (commits) => Array.isArray(commits?.commits)
  && !commits.commits_truncated && !commits.next_cursor;

export function resolveReviewCommitReference(selection, commits) {
  if (!hasReviewCommitPrefix(selection)) return { selection };
  if (selection.fileRef || !completeList(commits)) return { selection, pending: true };
  const prefix = selection.commit.toLowerCase();
  const matches = [...new Set(commits.commits.map(oidOf).filter((oid) => oid.length === 40 && oid.toLowerCase().startsWith(prefix)))];
  if (matches.length === 1) return { selection: { ...selection, commit: matches[0] } };
  return { selection: { ...selection, mode: "all", commit: null }, missing: matches.length ? "ambiguous" : "missing" };
}
