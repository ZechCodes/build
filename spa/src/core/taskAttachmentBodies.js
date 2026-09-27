// Task attachment bytes are fetched in ranges, persisted as byte pages, and
// read back from the cache before the lightbox creates its Blob URL.

import { createCachedBodies } from "./cachedBodies.js";
import { bytePagesOf, dropBodyPages } from "./bodyPages.js";
import { ATTACHMENT_BODY_MAX_BYTES, ATTACHMENT_RECORD_KIND } from "./cacheThresholds.js";
import { cachedAddresses, deleteCached, readCached, readCachedMany, recordWriteOf, writeCached } from "./localCache.js";

/** Bound the storage of large videos across a device and within each task.
 *  A revisit also expires their pages after 72 hours. */
export const TASK_VIDEO_RECORDS = 2;
export const DEVICE_VIDEO_RECORDS = 4;
export const TASK_VIDEO_TTL_MS = 72 * 60 * 60 * 1000;
export const TASK_VIDEO_META_KIND = "attachment-video";
const WIRE_RANGE_BYTES = 1_048_576;

const isLargeVideo = (record) => record.value?.mime?.startsWith("video/")
  && Number(record.value.size) > ATTACHMENT_BODY_MAX_BYTES;

function newestVideos(videos, taskId, preferredPath) {
  const fresh = videos.filter((record) => Date.now() - record.at < TASK_VIDEO_TTL_MS);
  const preferred = (record) => record.address.entityId === taskId && record.address.sub === preferredPath;
  fresh.sort((one, other) => Number(preferred(other)) - Number(preferred(one)) || other.at - one.at);
  const byTask = new Map();
  const keep = new Set();
  for (const record of fresh) {
    if (keep.size >= DEVICE_VIDEO_RECORDS) break;
    const task = record.address.entityId;
    const count = byTask.get(task) || 0;
    if (count >= TASK_VIDEO_RECORDS) continue;
    keep.add(record);
    byTask.set(task, count + 1);
  }
  return keep;
}

async function trimDeviceVideos(deviceId, taskId, preferredPath = null) {
  if (!deviceId || !taskId) return;
  // Metadata records are small. Inline attachment bodies and byte pages are
  // never deserialized by a sweep.
  const addresses = (await cachedAddresses({ deviceId }))
    .filter((address) => address.kind === TASK_VIDEO_META_KIND);
  const records = await readCachedMany(addresses);
  const videos = records.flatMap((record, index) => record ? [{ ...record, address: addresses[index] }] : []).filter(isLargeVideo);
  const keep = newestVideos(videos, taskId, preferredPath);
  for (const record of videos) {
    if (keep.has(record)) continue;
    const head = { ...record.address, kind: ATTACHMENT_RECORD_KIND };
    await deleteCached([record.address, head]);
    await dropBodyPages(head);
  }
}

async function markLargeVideo(deviceId, taskId, path, body) {
  if (!deviceId || !taskId || !isLargeVideo({ value: body })) return false;
  const head = { deviceId, entityId: taskId, kind: ATTACHMENT_RECORD_KIND, sub: path };
  const stored = await readCached(head);
  if (!stored?.value?.paged) return false;
  const marker = { ...head, kind: TASK_VIDEO_META_KIND };
  const headWrite = recordWriteOf(stored);
  if ((await readCached(marker))?.value?.headWrite === headWrite) return false;
  await writeCached(marker, { mime: body.mime, size: body.size, headWrite });
  return true;
}

async function trimNewVideo(deviceId, taskId, path, body) {
  if (await markLargeVideo(deviceId, taskId, path, body)) await trimDeviceVideos(deviceId, taskId, path);
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

/** One task's attachment loader. `load` returns `{ mime, size, pages }`, with
 *  each page a base64 string the shared Blob helper decodes independently. */
export function createTaskAttachmentBodies({ deviceId, taskId, call }) {
  const answered = new Map();
  const bodies = createCachedBodies({
    addressOf: (path) => deviceId && taskId
      ? { deviceId, entityId: taskId, kind: ATTACHMENT_RECORD_KIND, sub: path }
      : null,
    fetchMissing: (paths) => Promise.all(paths.map(async (path) => ({
      path,
      body: await readAttachmentPages((offset) => call("tasks.attachment", {
        task_id: taskId, path, ...(offset ? { offset } : {}), length: WIRE_RANGE_BYTES,
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

  let swept;
  return {
    async load(path) {
      swept ||= trimDeviceVideos(deviceId, taskId);
      await swept;
      if (!bodies.has(path)) await bodies.ensure([path]);
      let body = bodies.read(path) ?? answered.get(path);
      if (!hasAllBytes(body)) {
        await bodies.ensure([path]);
        body = bodies.read(path) ?? answered.get(path);
      }
      answered.delete(path);
      if (!hasAllBytes(body)) throw new Error("This attachment could not be loaded completely.");
      await trimNewVideo(deviceId, taskId, path, body);
      const size = Number(body.size) || 0;
      const pages = Array.isArray(body.content_b64)
        ? body.content_b64
        : splitPieces([body.content_b64 || ""], size).map((page) => page.body);
      return { mime: body.mime, size, pages };
    },
    forget: (path) => {
      answered.delete(path);
      return bodies.forget(path);
    },
    dispose: () => {
      answered.clear();
      bodies.dispose();
    },
  };
}
