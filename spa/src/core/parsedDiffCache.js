// Incremental unified-diff parsing. Aggregate review payloads still carry one
// complete patch because comment anchors depend on it, but parsing
// is isolated per file so an edit in one file does not rebuild every row model.

import { parseDiff } from "./diff.js";
import { fileViewFromParsedFile } from "./fileEntries.js";

const DEFAULT_LIMIT = 400;

/** Split a unified patch at file headers, retaining each complete file patch. */
export function filePatches(patch) {
  const parts = [];
  let current = [];
  for (const line of String(patch || "").split("\n")) {
    if (line.startsWith("diff --git ")) {
      if (current.length) parts.push(current.join("\n"));
      current = [line];
    } else if (current.length) current.push(line);
  }
  if (current.length) parts.push(current.join("\n"));
  return parts;
}

const pathFromPatch = (patch) => parseDiff(patch)[0]?.path || "?";
const parseIdentity = (contentKeys, revision) =>
  revision == null ? JSON.stringify(contentKeys) : String(revision);

const patchPath = (filePatch) => {
  const match = filePatch.split("\n", 1)[0].match(/ b\/(.+)$/);
  return match ? match[1] : pathFromPatch(filePatch);
};

/** A bounded per-file parse cache. `contentKeys` should be bridge revisions
 * when supplied; the complete file patch is the compatibility key otherwise. */
export function createParsedDiffCache({ limit = DEFAULT_LIMIT } = {}) {
  const held = new Map();
  let lastPatch = null;
  let lastIdentity = null;
  let lastViews = null;
  let lastKeys = null;
  let lastStampIdentity = null;
  let lastStampedViews = null;

  const remember = (key, value) => {
    held.delete(key);
    held.set(key, value);
    while (held.size > limit) held.delete(held.keys().next().value);
    return value;
  };

  const touch = (keys) => {
    for (const key of keys) {
      const view = held.get(key);
      if (view) remember(key, view);
    }
  };

  const readView = (filePatch, contentKeys, revision) => {
    const path = patchPath(filePatch);
    const contentKey = contentKeys[path] || (revision == null ? filePatch : revision);
    const cacheKey = `${path}\x01${contentKey}`;
    const cached = held.get(cacheKey);
    if (cached) return { key: cacheKey, view: remember(cacheKey, cached) };
    const parsed = parseDiff(filePatch)[0];
    // A malformed/incomplete file must never poison the cache. A later
    // complete payload with the same bridge revision still gets parsed.
    if (!parsed) return { key: cacheKey, view: null };
    return { key: cacheKey, view: remember(cacheKey, fileViewFromParsedFile(parsed)) };
  };

  const views = (patch, { contentKeys = {}, editedAt = {}, revision = null, defaultEditedAt } = {}) => {
    const identity = parseIdentity(contentKeys, revision);
    const stampIdentity = `${JSON.stringify(editedAt)}\x01${String(defaultEditedAt ?? "")}`;
    const stamp = (view) => ({ ...view, editedAt: editedAt[view.path] ?? defaultEditedAt });
    // Viewport and keyed-list repaints frequently ask for the exact same
    // aggregate. Keep that path O(files): splitting a multi-megabyte patch and
    // hashing every row again was still visible even after per-file caching.
    if (patch === lastPatch && identity === lastIdentity && lastViews) {
      touch(lastKeys);
      if (stampIdentity === lastStampIdentity) return lastStampedViews;
      lastStampIdentity = stampIdentity;
      lastStampedViews = lastViews.map(stamp);
      return lastStampedViews;
    }
    const parsed = filePatches(patch).map((filePatch) => readView(filePatch, contentKeys, revision));
    const parsedKeys = parsed.map((entry) => entry.key);
    const parsedViews = parsed.map((entry) => entry.view).filter(Boolean);
    lastPatch = patch;
    lastIdentity = identity;
    lastViews = parsedViews;
    lastKeys = parsedKeys;
    lastStampIdentity = stampIdentity;
    lastStampedViews = parsedViews.map(stamp);
    return lastStampedViews;
  };

  return {
    views,
    clear: () => {
      held.clear();
      lastPatch = null;
      lastIdentity = null;
      lastViews = null;
      lastKeys = null;
      lastStampIdentity = null;
      lastStampedViews = null;
    },
  };
}
