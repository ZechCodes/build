// The bytes of an issue's attachments, painted from the cache (#116).
//
// An attachment is content-addressed and never changes, so once its bytes are
// in the local cache a revisit paints the picture without asking the bridge.
// The cache is the one place they are read from: a fetch writes through, and
// the page reads the stored record back (core/cachedBodies.js).
//
// A recording can be ten times what one DataChannel message carries, so the
// bridge hands it back in pieces (`issues.attachment` `offset`, 1.18) and
// this puts them back together. A bridge older than ranges ignores `offset`
// and answers the whole file, which the same loop reads as one piece.
//
// A body over `ATTACHMENT_BODY_MAX_BYTES` is painted from the answer and never
// stored — #94's oversized-body rule, which createCachedBodies applies — so a
// long recording costs this mount its bytes and not the disk.

import { createCachedBodies } from "./cachedBodies.js";
import { ATTACHMENT_BODY_MAX_BYTES, ATTACHMENT_RECORD_KIND } from "./cacheThresholds.js";

const bytesOf = (b64) => Uint8Array.from(atob(b64 || ""), (char) => char.charCodeAt(0));

/** Base64 of a byte array, a slice at a time: one `fromCharCode` over tens of
 *  megabytes overflows the argument limit. */
function base64Of(bytes) {
  const SLICE = 0x8000;
  let binary = "";
  for (let at = 0; at < bytes.length; at += SLICE) {
    binary += String.fromCharCode(...bytes.subarray(at, at + SLICE));
  }
  return btoa(binary);
}

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
  const bodies = createCachedBodies({
    addressOf: (path) =>
      deviceId && issueId ? { deviceId, entityId: issueId, kind: ATTACHMENT_RECORD_KIND, sub: path } : null,
    fetchMissing: (paths) =>
      Promise.all(paths.map(async (path) => ({
        path,
        body: await readAttachmentWhole((offset) =>
          call("issues.attachment", { issue_id: issueId, path, ...(offset ? { offset } : {}) })),
      }))),
    valueOf: ({ path, body }) => ({
      key: path,
      value: { mime: body.mime, size: Number(body.size) || 0, content_b64: body.content_b64 || "" },
    }),
    cacheable: (body) => body.size <= ATTACHMENT_BODY_MAX_BYTES,
  });

  return {
    async load(path) {
      if (!bodies.has(path)) await bodies.ensure([path]);
      const body = bodies.read(path);
      if (!body) throw new Error("This attachment could not be loaded.");
      return body;
    },
    dispose: bodies.dispose,
  };
}
