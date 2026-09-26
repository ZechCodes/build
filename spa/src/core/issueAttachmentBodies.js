// The bytes of an issue's attachments, painted from the cache (#116).
//
// An attachment is content-addressed and never changes, so once its bytes are
// in the local cache a revisit paints the picture without asking the bridge.
// The cache is the one place they are read from: a fetch writes through, and
// the page reads the stored record back (core/cachedBodies.js).
//
// A recording can be ten times what one DataChannel message carries, so the
// bridge hands it back in pieces (`issues.attachment` `offset`, 1.19) and
// this puts them back together. A bridge older than ranges ignores `offset`
// and answers the whole file, which the same loop reads as one piece.
//
// A body over `ATTACHMENT_BODY_MAX_BYTES` is kept in pages (#95,
// core/bodyPages.js): one record per quarter megabyte, so a long recording
// never sits under a single record, and the page still paints it from the
// cache. The bytes arrive whole (the pieces above), so they are split here.

import { createCachedBodies } from "./cachedBodies.js";
import { base64Of, bytePagesOf, bytesOfBase64 as bytesOf, joinedBase64 } from "./bodyPages.js";
import { ATTACHMENT_BODY_MAX_BYTES, ATTACHMENT_RECORD_KIND } from "./cacheThresholds.js";

/** Every piece, joined. A file that fits one answer comes back as that answer
 *  untouched — the common case costs no decode. */
function joined(first, pieces, size) {
  if (pieces.length === 1) return first;
  const whole = new Uint8Array(size);
  let at = 0;
  for (const piece of pieces) {
    whole.set(piece.subarray(0, size - at), at);
    at += piece.length;
  }
  return { ...first, offset: 0, content_b64: base64Of(whole) };
}

/**
 * Read one attachment whole. `read(offset)` answers one `issues.attachment`
 * piece. Stops at `size`, and on a piece that carries nothing, so a bridge
 * that answers short can never hold the loop.
 */
export async function readAttachmentWhole(read) {
  const first = await read(0);
  const size = Number(first?.size) || 0;
  const pieces = [];
  let answer = first;
  let held = 0;
  for (;;) {
    const piece = bytesOf(answer?.content_b64);
    pieces.push(piece);
    held += piece.length;
    if (!piece.length || held >= size) break;
    answer = await read(held);
  }
  if (pieces.length === 1) return first;
  return joined(first, pieces, Math.min(size, held));
}

/**
 * One issue page's attachment bodies. `load(path)` answers `{ mime, size,
 * content_b64 }` — from the cache when it holds them, from the bridge
 * otherwise — and is the loader `wireThreadAttachments` is handed.
 */
export function createIssueAttachmentBodies({ deviceId, issueId, call }) {
  // What the bridge answered, held for this mount only until the stored
  // record is read back. A browser with no IndexedDB — a private window, a
  // locked-down profile — stores nothing, and a picture the bridge handed over
  // must still be drawn; this is the answer it is drawn from.
  const answered = new Map();
  const bodies = createCachedBodies({
    addressOf: (path) =>
      deviceId && issueId ? { deviceId, entityId: issueId, kind: ATTACHMENT_RECORD_KIND, sub: path } : null,
    fetchMissing: (paths) =>
      Promise.all(paths.map(async (path) => ({
        path,
        body: await readAttachmentWhole((offset) =>
          call("issues.attachment", { issue_id: issueId, path, ...(offset ? { offset } : {}) })),
      }))),
    valueOf: ({ path, body }) => {
      const value = { mime: body.mime, size: Number(body.size) || 0, content_b64: body.content_b64 || "" };
      answered.set(path, value);
      return { key: path, value };
    },
    cacheable: (body) => body.size <= ATTACHMENT_BODY_MAX_BYTES,
    pages: {
      field: "content_b64",
      split: (body, of) => bytePagesOf(body.content_b64, { of, total: body.size }),
      join: joinedBase64,
    },
  });

  return {
    async load(path) {
      if (!bodies.has(path)) await bodies.ensure([path]);
      // The cache first, always; the answer only when persistence kept nothing.
      const body = bodies.read(path) ?? answered.get(path);
      answered.delete(path);
      if (!body) throw new Error("This attachment could not be loaded.");
      return body;
    },
    dispose: () => {
      answered.clear();
      bodies.dispose();
    },
  };
}
