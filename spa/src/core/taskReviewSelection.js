// Review selections are shareable addresses. These pure mappings never read
// per-device preferences or connection state.
const MODES = new Set(["all", "since", "commit"]);
const VIEWS = new Set(["changes", "files", "comments"]);
const positiveInteger = (value) => Number.isSafeInteger(Number(value)) && Number(value) > 0;
const validSide = (value) => value === "old" || value === "new";
const safePath = (path) => typeof path === "string" && !path.startsWith("/")
  && path.split("/").every((part) => part && part !== "." && part !== "..");

/** Parse the already-decoded tail after a tracker task's id. */
export function parseReviewRoute(tail) {
  const [prefix, revisionText, mode, ...rest] = tail;
  if (prefix !== "review" || !/^r[1-9]\d*$/.test(revisionText) || !MODES.has(mode)) return null;
  const revision = Number(revisionText.slice(1));
  if (!positiveInteger(revision) || (mode === "commit" && !rest[0])) return null;
  const [directoryId, ...path] = mode === "commit" ? rest.slice(1) : rest;
  return { review: { revision, mode, ...comparisonPath(mode, rest[0], directoryId, path) } };
}

const comparisonPath = (mode, commit, directoryId, path) => ({
  ...(mode === "commit" ? { commit } : {}), ...(directoryId ? { directoryId } : {}),
  ...(path.length ? { path: path.join("/") } : {}),
});

/** The optional place within a review comparison. Invalid line targets and
 * unrelated task-layout queries have no meaning here. */
export function reviewRouteQuery(query) {
  const params = new URLSearchParams(query || "");
  const line = Number(params.get("line"));
  const side = params.get("side");
  const view = params.get("view");
  const fileRef = params.get("fileRef");
  return { ...(fileRef ? { fileRef } : {}), ...(validSide(side) ? { side } : {}), ...(positiveInteger(line) ? { line } : {}),
    ...(VIEWS.has(view) && view !== "changes" ? { view } : {}) };
}

/** Build the review portion of a task URL, including its own query. */
export function reviewRoutePath(selection) {
  const review = routeReviewSelection(selection);
  if (!review) return "";
  const comparison = review.mode === "commit" ? `commit/${encodeURIComponent(review.commit)}` : review.mode;
  const directory = review.directoryId ? `/${encodeURIComponent(review.directoryId)}` : "";
  const path = directory && review.path ? `/${review.path.split("/").map(encodeURIComponent).join("/")}` : "";
  const params = new URLSearchParams();
  if (review.fileRef) params.set("fileRef", review.fileRef);
  if (review.side) params.set("side", review.side);
  if (review.line) params.set("line", String(review.line));
  if (review.view) params.set("view", review.view);
  const query = String(params);
  return `/review/r${review.revision}/${comparison}${directory}${path}${query ? `?${query}` : ""}`;
}

const snapshotOf = (review, selection) => review?.snapshots?.find((row) => row.number === Number(selection?.revision))
  || review?.snapshots?.find((row) => row.id === selection?.snapshotId) || review?.snapshots?.at(-1);
const directoryOf = (snapshot, id) => snapshot?.directories?.find((row) => row.id === id) || snapshot?.directories?.[0];
const defaultView = (directory) => directory?.status === "not_git" ? "files" : "changes";
const modeOf = (selection) => MODES.has(selection?.mode) && (selection.mode !== "commit" || selection.commit) ? selection.mode : "all";
const anchorOf = ({ path, side, line }) => path && validSide(side) && positiveInteger(line) ? { path, side, line: Number(line) } : null;

/** Restore a cached review with either an explicit URL selection or its latest
 * revision. Keeping this result in the URL pins the revision as new ones land. */
export function selectionForReview(review, selection = {}) {
  const snapshot = snapshotOf(review, selection);
  if (!snapshot) return null;
  const directory = directoryOf(snapshot, selection.directoryId);
  const place = normalizedPlace(selection);
  const mode = modeOf(selection);
  return { snapshotId: snapshot.id, revision: snapshot.number, directoryId: directory?.id || null,
    mode, commit: mode === "commit" ? selection.commit : null,
    view: VIEWS.has(selection.view) ? selection.view : defaultView(directory),
    ...place, anchor: anchorOf(place) };
}

function normalizedPlace(selection) {
  return { path: safePath(selection.path) ? selection.path : "",
    side: validSide(selection.side) ? selection.side : null,
    line: positiveInteger(selection.line) ? Number(selection.line) : null };
}

/** Drop derived cache IDs and defaults before assigning `route.review`. */
export function routeReviewSelection(selection) {
  if (!positiveInteger(selection?.revision)) return null;
  const mode = modeOf(selection);
  return { revision: Number(selection.revision), mode,
    ...(mode === "commit" ? { commit: selection.commit } : {}),
    ...(selection.directoryId ? { directoryId: selection.directoryId } : {}), ...routePlace(selection) };
}

function routePlace(selection) {
  const { path, side, line } = normalizedPlace(selection);
  return { ...(selection.fileRef ? { fileRef: selection.fileRef } : {}), ...(path ? { path } : {}), ...(side ? { side } : {}), ...(line ? { line } : {}),
    ...(VIEWS.has(selection.view) && selection.view !== "changes" ? { view: selection.view } : {}) };
}

function unreadableCommit(commit, commits) {
  if (!commits) return false;
  if (commits.read_error) return true;
  const rows = Array.isArray(commits) ? commits : commits.commits;
  if (!Array.isArray(rows)) return false;
  if (rows.some((row) => (row.oid || row.sha || row.hash || row.commit) === commit)) return false;
  return !commits.commits_truncated && !commits.next_cursor;
}

/** Anchors from old comments retain their range meaning. A commit that is not
 * yet cached is tentative; an explicit complete list or read error can prove
 * it unreadable and route straight to the range instead. */
export function reviewAnchorSelection(review, anchor, { commits = null, support = {} } = {}) {
  const snapshot = anchorSnapshot(review, anchor);
  if (!snapshot) return null;
  const commit = readableAnchorCommit(anchor, support, commits);
  return selectionForReview(review, { revision: snapshot.number, mode: commit ? "commit" : "all", commit,
    directoryId: anchor.directory_id, path: anchor.path, side: anchor.side, line: anchor.line, view: "changes" });
}

function anchorSnapshot(review, anchor) {
  const snapshot = review?.snapshots?.find((row) => row.id === anchor?.snapshot_id);
  return snapshot?.directories?.some((row) => row.id === anchor.directory_id) ? snapshot : null;
}

const readableAnchorCommit = (anchor, support, commits) =>
  anchor.commit && support.commit && !unreadableCommit(anchor.commit, commits) ? anchor.commit : null;

/** Most transitions preserve the current path and line. A loaded commit's
 * complete file list clears a file that did not change in that commit. */
export function switchReviewSelection(selection, changes, { files } = {}) {
  const next = { ...selection, ...changes };
  const comparisonChanged = changes.mode === "commit" || Object.hasOwn(changes, "commit");
  if (comparisonChanged && Array.isArray(files) && !files.some((file) => file.path === next.path)) {
    Object.assign(next, { path: "", side: null, line: null });
  }
  if (next.mode !== "commit") next.commit = null;
  next.anchor = anchorOf(next);
  return next;
}
