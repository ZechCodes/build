// How long the cache keeps what a workspace holds (the owner's rule, 2026-09-18).
//
// A workspace's data — its status, commits, diffs, trees, terminals, threads,
// file bodies, the surfaces and the console's tab pick — is worth holding for
// as long as the reader might come back to it, and worth nothing once they
// cannot. So:
//
//   finished or deleted   everything goes at once, the moment the board says so
//   only recent           it ages out 72 h after its last write
//   active                it never expires
//
// The one thing that stays is the feed row: it is the board's to list and the
// board's to remove, and a workspace with no data still has a line on the
// inbox until the feed stops naming it. Nothing here decides which workspaces
// are active or recent — the caller knows that, and calls accordingly.

import { cachedAddresses, cachedAddressesWrittenBefore, cachedRecords, deleteCached, mergeCachedIfUnwritten, writeCached } from "./localCache.js";
import {
  BODY_PAGE_BYTES,
  bytePagesOf,
  dropBodyPages,
  pageFollows,
  pageFromAnswer,
  readBodyPages,
  writeBodyPage,
  writeBodyPages,
} from "./bodyPages.js";
import { bridgeCapabilities } from "./changeEvents.js";
import { fileBodyReading } from "./fileViewer.js";

export const WORKSPACE_DATA_TTL_MS = 72 * 60 * 60 * 1000;

/** File bodies kept per workspace, newest opened first. */
export const RECENT_FILES = 5;

/** The largest file body one record keeps. Applied by `cacheFileBody` below,
 *  which is the only way a file body gets into the store: a body over it is
 *  kept as pages (#95). */
export const FILE_MAX_BYTES = 1048576;

/** The most of a file the viewer shows whole (an image, a film, an HTML page)
 *  is read into pages for. The one-mebibyte pages keep each reply below the
 *  transport limit while this bounds cache and phone memory. */
export const FILE_MEDIA_MAX_BYTES = 64 * 1048576;

/** A workspace can retain one large media body plus its four other recent
 * files. This bounds the expensive end of the existing five-file policy. */
export const RECENT_LARGE_MEDIA = 1;
const LARGE_MEDIA_BYTES = 16 * 1048576;

/** What the pages of an answer split here are of: a whole read names no
 *  version of the file, so a bridge's page — which does — never joins them. */
export const WHOLE_READ = "whole";

export const FILE_RECORD_KIND = "file";

/** The kinds under a workspace that are not its data. Written as what is kept
 *  rather than what goes: a kind added later that nobody thought to list here
 *  should expire with the workspace, not outlive it forever. */
const KEPT_KINDS = new Set(["row"]);

export const isWorkspaceDataKind = (kind) => !KEPT_KINDS.has(kind);

const workspaceData = (addresses) => addresses.filter((address) => isWorkspaceDataKind(address.kind));

/** Drop these addresses, and answer them. */
const drop = async (addresses) => {
  await deleteCached(addresses);
  return addresses;
};

/** Age out one recent workspace's data: everything last written more than the
 *  TTL ago. Called for a workspace the board still lists but nobody is on.
 *
 *  The read and the delete are two transactions, so a record rewritten in
 *  between is dropped on the age it had when the sweep started. That costs
 *  the reader a cold read of something just synced, which the next sync pass
 *  refills — the other way round, holding a readwrite transaction open over
 *  the whole sweep, would block the writers this cache exists to serve. */
export async function expireWorkspaceData(deviceId, entityId, now = Date.now(), active = () => true) {
  const stale = await cachedAddressesWrittenBefore({ deviceId, entityId }, now - WORKSPACE_DATA_TTL_MS);
  if (!active()) return [];
  return drop(workspaceData(stale));
}

/** Let go of one workspace's data at once — it is done, or deleted. The feed
 *  row stays: the board is what removes a row. */
export async function evictWorkspaceData(deviceId, entityId, active = () => true) {
  const addresses = await cachedAddresses({ deviceId, entityId });
  if (!active()) return [];
  return drop(workspaceData(addresses));
}

/** When a file body was last opened — the writer stamps it; a record written
 *  without one counts as opened when it was written. */
const openedAt = (record) =>
  typeof record.value?.openedAt === "number" ? record.value.openedAt : record.at;

/** Keep one workspace's five most recently opened file bodies and drop the
 *  rest. Called after a file is read into the cache. */
export async function trimRecentFiles(deviceId, entityId) {
  const files = await cachedRecords({ deviceId, entityId, kind: FILE_RECORD_KIND });
  const newestFirst = [...files].sort((one, other) => openedAt(other) - openedAt(one));
  let largeMedia = 0;
  const doomed = newestFirst.filter((record, index) => {
    const file = record.value?.file;
    const large = fileBodyReading(file?.mime) === "media" && Number(file?.size) > LARGE_MEDIA_BYTES;
    if (large) largeMedia += 1;
    return index >= RECENT_FILES || (large && largeMedia > RECENT_LARGE_MEDIA);
  });
  const dropped = await drop(doomed.map((record) => record.address));
  // A paged body's bytes are records of their own, and go with it.
  for (const head of dropped) await dropBodyPages(head);
  return dropped;
}

/** How a body is measured against a cap: in bytes on the wire, never in
 *  characters. A source file of Japanese is three bytes a character, so a
 *  cap read off `length` lets three times the rule onto the disk. Nothing to
 *  measure is within every cap.
 *
 *  A body whose character count alone is over the cap is answered without
 *  encoding it: UTF-8 is never shorter than the UTF-16 length, so that
 *  comparison is already decisive, and the encode it saves is the one that
 *  would have allocated the oversized copy this check exists to refuse. */
export function withinBytes(text, maxBytes) {
  if (text === null || text === undefined) return true;
  const body = String(text);
  if (body.length > maxBytes) return false;
  return new TextEncoder().encode(body).length <= maxBytes;
}

/** Whether a read answer fits one record.
 *
 *  Measured against the file's own size — the bytes on disk, which is what the
 *  rule is about — rather than against the body on the wire, which is base64
 *  and a third bigger than them. A read that came back truncated never fits
 *  whatever it weighs: it is not the file, and kept as one it would open again
 *  as the whole of one, with nothing on screen saying which piece. */
const fileBodyFits = (file) => {
  if (file.truncated) return false;
  // `Number(null)` is zero and passes every cap, so the field has to be a
  // number before it is read as one: an answer that names no size is measured
  // by what it carries.
  const sized = typeof file.size === "number" && Number.isFinite(file.size);
  return sized ? file.size <= FILE_MAX_BYTES : withinBytes(file.content_b64, FILE_MAX_BYTES);
};

/** Whether `deviceId`'s bridge can page (#95). A bridge that cannot is never
 *  sent a `range`: it would refuse one. Asked by a store deciding what to do
 *  with an answer, which a greeting has always come before; never by a view. */
export const filePagesReadable = (deviceId) => bridgeCapabilities(deviceId)?.bodies?.pages === true;

/** A reader of one page of a file: `read(range)` asks `fs.read` for the file
 *  with that `range`. Whether the bridge can page is asked at each read, not
 *  when the reader is made, and a read it cannot make answers null — "not
 *  yet" — so a view painted from the cache before its machine's greeting
 *  reads on once greeted, with nothing painted again. */
export function filePageReader(deviceId, read, mime = "") {
  return async (offset, bytes = BODY_PAGE_BYTES) =>
    filePagesReadable(deviceId) ? pageFromAnswer(await read({
      offset, bytes,
      ...(fileBodyReading(mime) === "media" && bridgeCapabilities(deviceId)?.bodies?.mediaRawPages ? { raw: true } : {}),
    }), "content_b64") : null;
}

/** What a file too large for one record is, without its body or a revision:
 *  never offered for editing, since what the reader holds is not the file. */
const fileFacts = (file, truncated) => ({
  path: file.path,
  size: file.size,
  mime: file.mime,
  truncated,
  editable: false,
});

/** The record of a paged file: its facts, and the version its pages are of. */
const pagedHead = (file, over) => ({ ...fileFacts(file, false), paged: true, ...over });

/** The record of a file the viewer shows as its size alone — a binary, or a
 *  film past the media cap — with no bytes beside it. */
async function bodilessHead(head, file, truncated) {
  await dropBodyPages(head);
  return fileFacts(file, truncated);
}

/** How much one page read asks for: a source file is painted a page at a
 *  time, so its pages are small; a body painted whole is read in the
 *  largest pages the bridge cuts, for the fewest round trips. */
const pageBytesFor = (reading) => (reading === "lines" ? BODY_PAGE_BYTES : FILE_MAX_BYTES);

const readWhole = (reading) => reading === "media" || reading === "rendered";

/** Read the pages after those held of `of`, to the end: a body the viewer
 *  shows whole. A page of another version means the file changed under the
 *  read; the pages stop there, and the viewer's own reader starts it over. */
async function readRemainingPages(head, first, readPage) {
  let { end, complete } = await readBodyPages(head, first.of);
  while (!complete) {
    const page = await readPage(end, FILE_MAX_BYTES);
    if (!pageFollows(page, end, first.of)) return;
    await writeBodyPage(head, page);
    end = page.end;
    complete = end >= page.total;
  }
}

/** A body over one record, from a bridge that pages: its first page read by
 *  range — never the whole answer, which names no version — and every page of
 *  one the viewer shows whole. Answers the record, or null when the reader
 *  had nothing to give (a read given up on). */
async function readFilePages(head, file, readPage) {
  const reading = fileBodyReading(file.mime);
  if (reading === "none") return bodilessHead(head, file, false);
  const whole = readWhole(reading);
  if (whole && file.size > FILE_MEDIA_MAX_BYTES) return bodilessHead(head, file, true);
  const first = await readPage(0, pageBytesFor(reading));
  if (!first) return null;
  if (whole && first.total > FILE_MEDIA_MAX_BYTES) return bodilessHead(head, { ...file, size: first.total }, true);
  await writeBodyPage(head, first);
  if (whole) await readRemainingPages(head, first, readPage);
  return pagedHead(file, { size: first.total, of: first.of });
}

/** A body over one record, from a bridge that cannot page: what the answer
 *  carried, split into pages that weigh the file's own size, so a cut answer
 *  shows as the piece it is. A cut film is worth nothing, and not kept. */
async function splitWholeAnswer(head, file) {
  const reading = fileBodyReading(file.mime);
  if (reading === "none" || (reading === "media" && file.truncated)) return bodilessHead(head, file, Boolean(file.truncated));
  const pages = bytePagesOf(file.content_b64, { of: WHOLE_READ, total: file.truncated ? file.size : undefined });
  // Two whole reads name the same version, so the pages of the last one would
  // chain on past the end of a shorter body: they go first.
  await dropBodyPages(head);
  await writeBodyPages(head, pages);
  return pagedHead(file, { size: pages[0].total, of: WHOLE_READ, truncated: Boolean(file.truncated) });
}

/** What the file's own record holds, with whatever pages it needs put beside
 *  it first. A paged record handed back (a reopen) is kept as it is, or
 *  refreshed from its first page when there is a reader to read it with. */
async function fileRecordOf(head, file, readPage) {
  if (file.paged) return readPage ? readFilePages(head, file, readPage) : file;
  if (fileBodyFits(file)) {
    await dropBodyPages(head);
    return file;
  }
  return readPage ? readFilePages(head, file, readPage) : splitWholeAnswer(head, file);
}

/** `readPage`, answering no page once the record it reads for is not the
 *  one it started from (`still`); as it is where nothing is asked. */
const readingWhileStill = (readPage, still) => readPage && still ? async (...range) => {
  const page = await readPage(...range);
  return page && (await still()) ? page : null;
} : readPage;

/** Whether nothing asks, or the record is still the one started from. */
const stillStands = async (still) => !still || still();

/** Put the record — with `written`, only while it is still that write — and
 *  then hold the recent-files rule. Answers whether it was put. */
const putFileRecord = async (head, value, written) => {
  if (written === undefined) await writeCached(head, value);
  else if (!(await mergeCachedIfUnwritten(head, written, () => value))) return false;
  await trimRecentFiles(head.deviceId, head.entityId);
  return true;
};

/** Put one file's body in the cache, under both of the owner's rules for it:
 *  a body over `FILE_MAX_BYTES` is kept as pages beside a record saying what
 *  the file is, and a write leaves at most `RECENT_FILES` bodies behind it.
 *  `file` is the `fs.read` answer as it came, or a paged record held before.
 *  `readPage(offset, bytes)` reads one page by range (see `pageFromAnswer`)
 *  where the file's bridge can page, and is null where it cannot. Answers
 *  whether the record was written.
 *
 *  Every file body goes in through here. A writer that wrote the record
 *  itself would be a second place the two rules have to be remembered, and
 *  the one that forgot them would put a 40 MB body under a 1 MB row of the
 *  Records table.
 *
 *  The pages are written before the record, so a viewer that hears of the
 *  record finds the bytes it names already there.
 *
 *  `still()`, where given, says whether the record is still the one the
 *  caller started from. A page is a wire call, and a record written or
 *  dropped while one was out is left as it is now: no page it brings is
 *  kept — the first would let go of the newer record's pages — and no record
 *  is written over it.
 *
 *  `written`, where given, names the write (`cachedWriteOf`) the caller read
 *  the record at, and the record is put only if it is still that write,
 *  checked in the same transaction as the put: a push landing between a
 *  `still()` check and the write would otherwise be written over. */
export async function cacheFileBody({ deviceId, entityId, path, file, openedAt = Date.now(), readPage = null, still = null, written }) {
  if (!deviceId || !entityId || !file) return false;
  const head = { deviceId, entityId, kind: FILE_RECORD_KIND, sub: path || "" };
  const record = await fileRecordOf(head, file, readingWhileStill(readPage, still));
  if (!record || !(await stillStands(still))) return false;
  return putFileRecord(head, { file: record, openedAt }, written);
}
