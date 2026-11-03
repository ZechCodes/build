import { fileKey, pathOf } from "./diff.js";

/** A filtered patch is never a whole-commit cache entry. A structured subkey
 * keeps colons and other legal path characters separate from the commit. */
export const filteredCommitPatchKey = (hash, path) => JSON.stringify([hash, path]);
export function commitPatchParams(key) {
  if (!key.startsWith("[")) return { hash: key };
  const [hash, path] = JSON.parse(key);
  return { hash, paths: [path] };
}

export function createCommitDiffTarget(commit, place) {
  let pending = Boolean(place?.path);
  let scrolledTo = null;
  let pagesSought = 0;
  let preparedKey = null;
  const matches = (hash) => Boolean(pending && hash && hash.toLowerCase().startsWith(String(commit).toLowerCase()));
  return {
    pathFor: (hash) => matches(hash) ? place.path : null,
    select(hash) { pending = matches(hash); },
    prepare(hash, views, folds, viewport, noiseExpanded) {
      if (!pending || !matches(hash)) return;
      const file = views.find((view) => view.path === place.path);
      if (!file) return;
      const key = fileKey(file);
      const side = place.side === "old" ? "o" : "n";
      const row = file.rows.findIndex((row) => row[side] === place.line);
      const prepared = `${key}:${row}`;
      if (preparedKey === prepared) return;
      preparedKey = prepared;
      folds.openBody(key);
      noiseExpanded.add(String(hash));
      viewport.reveal(key, Math.max(0, row));
    },
    needsMore(hash, views, pages) {
      if (!matches(hash) || !pages || pages.complete || pagesSought >= 200) return false;
      const file = views.find((view) => view.path === place.path);
      const side = place.side === "old" ? "o" : "n";
      const found = file && (!place.line || file.rows.some((row) => row[side] === place.line));
      if (found) return false;
      pagesSought += 1;
      return true;
    },
    paint(hash, host) {
      if (!pending || !matches(hash)) return;
      const file = [...host.querySelectorAll(".file[data-key]")].find((file) => pathOf(file.dataset.key) === place.path);
      if (!file) return;
      const lineKey = place.side === "old" ? "oldLine" : "newLine";
      const row = [...file.querySelectorAll("tr[data-side]")].find((row) => Number(row.dataset[lineKey]) === place.line);
      const target = row || file;
      if (row) row.classList.add("diff-link-anchor");
      const next = row ? "line" : "file";
      if (scrolledTo !== next) target.scrollIntoView?.({ block: "center" });
      scrolledTo = next;
    },
  };
}
