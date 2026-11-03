// A reference can be followed before its saved revision has reached this device.
// Resolve its ambiguous colons against that revision, never today's workspace.
const decoded = (value) => { try { return decodeURIComponent(value); } catch { return value; } };
const relativePath = (path) => path && !path.startsWith("/")
  && path.split("/").every((part) => part && part !== "." && part !== "..");

function filePlace(snapshot, raw) {
  const directories = snapshot?.directories || [];
  const literal = directories.filter((dir) => raw.startsWith(`${dir.id}:`))
    .sort((a, b) => b.id.length - a.id.length)[0];
  if (literal) return { directoryId: literal.id, path: raw.slice(literal.id.length + 1) };
  const seam = raw.indexOf(":");
  if (seam < 0) return directories.length === 1 ? { directoryId: directories[0].id, path: raw } : null;
  const directory = directories.find((dir) => dir.id === decoded(raw.slice(0, seam)));
  return directory ? { directoryId: directory.id, path: raw.slice(seam + 1) } : null;
}

export function resolveReviewFileReference(review, selection) {
  if (!selection.fileRef) return { selection };
  if (!review) return { selection, pending: true };
  const snapshot = review.snapshots?.find((row) => row.number === Number(selection.revision));
  const place = filePlace(snapshot, selection.fileRef);
  const { fileRef: _fileRef, ...resolved } = selection;
  if (place && relativePath(place.path)) return { selection: { ...resolved, ...place } };
  return { selection: { revision: selection.revision, mode: "all" }, missing: true };
}
