// Issue attachment bytes are fetched in ranges, persisted as byte pages, and
// read back from the cache before the lightbox creates its Blob URL.

import { createCachedBodies } from "./cachedBodies.js";
import { BODY_PAGE_BYTES, bytePagesOf, dropBodyPages } from "./bodyPages.js";
import { ATTACHMENT_BODY_MAX_BYTES, ATTACHMENT_RECORD_KIND } from "./cacheThresholds.js";
import { cachedAddresses, deleteCached, readCachedMany } from "./localCache.js";

/** Bound the storage of large videos across a device and within each issue.
 *  A revisit also expires their pages after 72 hours. */
export const ISSUE_VIDEO_RECORDS = 2;
export const DEVICE_VIDEO_RECORDS = 4;
export const ISSUE_VIDEO_TTL_MS = 72 * 60 * 60 * 1000;

const isLargeVideo = (record) => record.value?.mime?.startsWith("video/")
  && Number(record.value.size) > ATTACHMENT_BODY_MAX_BYTES;

function newestVideos(videos, issueId, preferredPath) {
  const fresh = videos.filter((record) => Date.now() - record.at < ISSUE_VIDEO_TTL_MS);
  const preferred = (record) => record.address.entityId === issueId && record.address.sub === preferredPath;
  fresh.sort((one, other) => Number(preferred(other)) - Number(preferred(one)) || other.at - one.at);
  const byIssue = new Map();
  const keep = new Set();
  for (const record of fresh) {
    if (keep.size >= DEVICE_VIDEO_RECORDS) break;
    const issue = record.address.entityId;
    const count = byIssue.get(issue) || 0;
    if (count >= ISSUE_VIDEO_RECORDS) continue;
    keep.add(record);
    byIssue.set(issue, count + 1);
  }
  return keep;
}

async function trimDeviceVideos(deviceId, issueId, preferredPath = null) {
  if (!deviceId || !issueId) return;
  // Keys are cheap to enumerate; only attachment heads are deserialized. Page
  // records can hold most of a film and must never be read by the sweep.
  const addresses = (await cachedAddresses({ deviceId }))
    .filter((address) => address.kind === ATTACHMENT_RECORD_KIND);
  const records = await readCachedMany(addresses);
  const videos = records.map((record, index) => ({ ...record, address: addresses[index] })).filter(isLargeVideo);
  const keep = newestVideos(videos, issueId, preferredPath);
  for (const record of videos) {
    if (keep.has(record)) continue;
    await deleteCached([record.address]);
    await dropBodyPages(record.address);
  }
}

/** Split each wire answer separately. No base64 string spanning the file is
 *  constructed, including when a legacy bridge answers it in one message. */
function splitPieces(pieces, total, of = "whole") {
  const pages = [];
  let offset = 0;
  for (const piece of pieces) {
    for (const page of bytePagesOf(piece, { of, total })) {
      pages.push({ ...page, offset: offset + page.offset, end: offset + page.end });
    }
    offset = pages.at(-1)?.end || 0;
  }
  return pages;
}

/** Read all byte ranges before returning. The first response may be the whole
 *  file from a bridge without ranges; it then needs no second request. */
function partAt(answer, held, size) {
  if (Number(answer?.offset ?? held) !== held) throw new Error("Attachment page starts at the wrong offset.");
  const content = answer?.content_b64 || "";
  const length = atob(content).length;
  if (!length && held < size) throw new Error("Attachment download ended before the file was complete.");
  if (length > size - held) throw new Error("Attachment page exceeds the file size.");
  return { content, length };
}

export async function readAttachmentPages(read) {
  const first = await read(0);
  const size = Number(first?.size) || 0;
  const pieces = [];
  let answer = first;
  let held = 0;
  for (;;) {
    const { content, length } = partAt(answer, held, size);
    if (length) pieces.push(content);
    held += length;
    if (held >= size) break;
    answer = await read(held);
  }
  return { mime: first?.mime || "application/octet-stream", size, pages: splitPieces(pieces.length ? pieces : [""], size).map((page) => page.body) };
}

function hasAllBytes(body) {
  if (!body) return false;
  if (body.pages?.complete !== undefined) return body.pages.complete;
  const parts = Array.isArray(body.content_b64) ? body.content_b64 : [body.content_b64 || ""];
  return parts.reduce((size, part) => size + atob(part).length, 0) >= Number(body.size);
}

/** One issue's attachment loader. `load` returns `{ mime, size, pages }`, with
 *  each page a base64 string the shared Blob helper decodes independently. */
export function createIssueAttachmentBodies({ deviceId, issueId, call }) {
  const answered = new Map();
  const bodies = createCachedBodies({
    addressOf: (path) => deviceId && issueId
      ? { deviceId, entityId: issueId, kind: ATTACHMENT_RECORD_KIND, sub: path }
      : null,
    fetchMissing: (paths) => Promise.all(paths.map(async (path) => ({
      path,
      body: await readAttachmentPages((offset) => call("issues.attachment", {
        issue_id: issueId, path, ...(offset ? { offset } : {}), length: BODY_PAGE_BYTES,
      }, { priority: "background" })),
    }))),
    valueOf: ({ path, body }) => {
      const value = { mime: body.mime, size: body.size, content_b64: body.pages };
      answered.set(path, value);
      return { key: path, value };
    },
    cacheable: () => false,
    pages: {
      field: "content_b64",
      split: (body, of) => splitPieces(body.content_b64, body.size, of),
      join: (pages) => pages.map((page) => page.body),
    },
  });

  return {
    async load(path) {
      await trimDeviceVideos(deviceId, issueId);
      if (!bodies.has(path)) await bodies.ensure([path]);
      await trimDeviceVideos(deviceId, issueId, path);
      let body = bodies.read(path) ?? answered.get(path);
      if (!hasAllBytes(body)) {
        await bodies.ensure([path]);
        body = bodies.read(path) ?? answered.get(path);
      }
      answered.delete(path);
      if (!hasAllBytes(body)) throw new Error("This attachment could not be loaded completely.");
      const size = Number(body.size) || 0;
      const pages = Array.isArray(body.content_b64)
        ? body.content_b64
        : splitPieces([body.content_b64 || ""], size).map((page) => page.body);
      return { mime: body.mime, size, pages };
    },
    dispose: () => {
      answered.clear();
      bodies.dispose();
    },
  };
}
