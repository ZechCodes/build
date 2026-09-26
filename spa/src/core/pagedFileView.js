// A file too large for one cache record, painted from its pages (#95).
//
// The file's own record says what the file is; its bytes are page records
// beside it (core/bodyPages.js). This paints what the cache holds, never an
// answer. Source is painted a page at a time: each page's lines are appended
// under the rows already painted, numbered on from them, and a sentinel under
// the last row says how much of the file that is. When the sentinel scrolls
// into view the next page is read into the cache, and the cache's
// announcement paints it. A line the page cuts — longer than a page, or a
// body split here — is painted as far as it has arrived, and the next page
// carries it on in the same row. A page that ends no line leaves the
// sentinel where it was, still in view, so the next is read only as the
// reader goes along that line: to its right-hand end, or by pressing the
// sentinel. A body the viewer can only show whole (a picture, a film, a
// rendered HTML page) is painted once every page is held.
//
// What is painted says only what the cache holds: how much of the file that
// is, never whether the machine can give more. A read it cannot make (an
// older bridge, or one not greeted yet) answers nothing and paints nothing;
// the next write to the pages — the sync layer's refresh once greeted —
// repaints, and the sentinel, if it is still in view, reads on.

import { bytesOfBase64, createPagedBody, joinedBase64 } from "./bodyPages.js";
import { highlightCode } from "./highlight.js";
import { readCached } from "./localCache.js";
import { humanBytes } from "./workspaceLifecycle.js";

/** How far below the scroller's edge the sentinel starts the next read, so
 *  the page is usually in before the reader gets there. */
const SENTINEL_MARGIN = "0px 0px 600px 0px";

/** Source rows, numbered from `first`: the one row shape a whole file and a
 *  paged one share, so a line reads the same either way. */
export const sourceRowsHtml = (lines, lang, first = 1) =>
  lines
    .map((line, index) => {
      const number = first + index;
      return `<tr data-new-line="${number}"><td class="fsrc-ln">${number}</td><td class="fsrc-code"><code>${highlightCode(line, lang) || " "}</code></td></tr>`;
    })
    .join("");

/** How near the right-hand end of a cut line the reader has to be for the
 *  rest of it to be read. */
const LINE_END_MARGIN_PX = 600;

/** The text of a body, a page at a time. A bridge cuts its pages after a line
 *  end, but a line longer than a page is cut inside it, and a body split here
 *  (read whole from an older bridge) is cut anywhere, even inside a
 *  character. So the bytes are decoded as one stream: `take` answers a page's
 *  text split at its line ends — the first piece carries on the line the page
 *  before left open — and `finish` what a cut character left over. */
function createLineReader() {
  const decoder = new TextDecoder();
  return {
    take: (page) => decoder.decode(bytesOfBase64(page.body), { stream: true }).split("\n"),
    finish: () => decoder.decode(),
  };
}

/** The text of every page held, decoded as one stream. */
export function pagesText(pages) {
  const decoder = new TextDecoder();
  return pages.map((page) => decoder.decode(bytesOfBase64(page.body), { stream: true })).join("") + decoder.decode();
}

/** Whether what is painted is where `state` goes on from: the same pages,
 *  and more of them only while the last line is still open. */
function paintedFollows(painted, state) {
  if (state.pages.length < painted.pages) return false;
  if (painted.pages && state.pages[painted.pages - 1].end !== painted.end) return false;
  return !painted.finished || state.pages.length === painted.pages;
}

/** Paints source a page at a time: the rows of a new page are appended, and
 *  the whole table is painted again only when the pages held are no longer
 *  the ones it shows (evicted, or of another version). The last line stays
 *  open until a line end or the body's end closes it: painted as far as it
 *  has arrived (`open`, its code cell), or not yet at all while it has no
 *  text, and each page's first piece is added on to it — so a line longer
 *  than any page is shown a page at a time, never held back whole.
 *
 *  `paint` answers false for pages that only carried the open line on — the
 *  table is no taller for them — and null when no page landed. */
export function sourceLinesPainter(lang) {
  let painted = null;
  const start = (content) => {
    content.innerHTML = `<div class="fsrc"><table></table></div>`;
    painted = { table: content.querySelector("table"), pages: 0, end: 0, lines: 0, open: null, finished: false, reader: createLineReader() };
  };
  const append = (lines) => {
    if (!lines.length) return null;
    const rows = painted.table.ownerDocument.createElement("tbody");
    rows.innerHTML = sourceRowsHtml(lines, lang, painted.lines + 1);
    painted.table.append(rows);
    painted.lines += lines.length;
    return rows.lastElementChild.querySelector("code");
  };
  /** The open line, carried on by `piece`. */
  const carryOn = (piece) => {
    if (!piece) return;
    if (!painted.open) {
      painted.open = append([piece]);
      return;
    }
    const more = painted.open.ownerDocument.createElement("span");
    more.innerHTML = highlightCode(piece, lang);
    painted.open.append(more);
  };
  /** The open line ends: one never painted is an empty line. */
  const close = () => {
    if (!painted.open) append([""]);
    painted.open = null;
  };
  const take = (pieces) => {
    carryOn(pieces[0]);
    if (pieces.length === 1) return false;
    close();
    const lines = pieces.slice(1, -1);
    append(lines);
    carryOn(pieces[pieces.length - 1]);
    return true;
  };
  return {
    paint(content, state, final) {
      const fresh = !painted || !paintedFollows(painted, state);
      if (fresh) start(content);
      const landed = state.pages.slice(painted.pages);
      let closed = fresh || (landed.length ? false : null);
      for (const page of landed) closed = take(painted.reader.take(page)) || closed;
      painted.pages = state.pages.length;
      painted.end = state.end;
      if (final && !painted.finished) {
        carryOn(painted.reader.finish());
        close();
        painted.finished = true;
      }
      return closed;
    },
  };
}

/** Paints the text of every page held in one piece, each time a page lands:
 *  for a view that has to see the whole text to draw any of it. */
export function wholeTextPainter(render) {
  return {
    paint(content, state) {
      render(content, pagesText(state.pages));
    },
  };
}

/** Paints a body the viewer shows whole — from every byte, once all of them
 *  are held — and only once per body, so a page written again under a
 *  playing film does not start it over. */
export function wholeBytesPainter(render) {
  let painted = null;
  return {
    paint(content, state) {
      const key = state.complete ? `${state.pages.length}:${state.end}` : null;
      if (key === painted) return;
      painted = key;
      if (key) render(content, joinedBase64(state.pages));
      else content.replaceChildren();
    },
  };
}

/**
 * Paint one paged file into `scroller` (the preview's scrolling body) from
 * the cache. `head` is the file's record address, `file` its record;
 * `readPage(offset)` reads the next page by range, answering null while it
 * cannot (see `filePageReader`). `restart()` is asked to read the file again
 * from its first page: a page read next was of a changed file, or no page of
 * this one is held any more. `painter` is one of the painters above, and
 * `onPaint()` hears every paint.
 *
 * Answers `{ more, dispose }`: `more()` reads the next page, which is what the
 * sentinel coming into view does.
 */
export function mountPagedFile(scroller, { head, file, readPage = null, restart = () => {}, painter, onPaint = () => {} }) {
  scroller.innerHTML = `<div class="fppages"></div><div class="fpmore" hidden></div><div class="ftrunc" hidden>truncated at 1 MiB</div>`;
  const content = scroller.querySelector(".fppages");
  const sentinel = scroller.querySelector(".fpmore");
  const notice = scroller.querySelector(".ftrunc");
  let observer = null;
  let disposed = false;

  // A page is only kept while the file's record still names the version it
  // is of: one landing after the recent-files rule or a newer store let go of
  // the record would be an orphan nothing reads or drops.
  const isCurrent = async () => {
    const held = (await readCached(head))?.value?.file;
    return held?.paged === true && held.of === file.of;
  };

  // The sentinel in view reads the next page, or — none held any more —
  // starts the file over. A read that lands nothing paints nothing, so the
  // sentinel is not watched again and cannot ask in a loop.
  const reachedEnd = () => {
    if (body.state().pages.length) void body.more();
    else restart();
  };

  // A page that closed no line left the sentinel where it was: it is not
  // watched again, or it would read the whole of one long line with the
  // reader standing still. The rest is read as they go along that line, or
  // press the sentinel.
  let parked = false;
  const readAlongTheLine = (event) => {
    const line = event.target;
    if (!parked || !line?.classList?.contains("fsrc")) return;
    if (line.scrollLeft + line.clientWidth >= line.scrollWidth - LINE_END_MARGIN_PX) reachedEnd();
  };
  scroller.addEventListener("scroll", readAlongTheLine, true);
  sentinel.addEventListener("click", reachedEnd);

  const watchSentinel = () => {
    if (!globalThis.IntersectionObserver) return;
    observer ??= new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) reachedEnd();
    }, { root: scroller, rootMargin: SENTINEL_MARGIN });
    // Observing it again reports where it stands now: a page too short to
    // push the sentinel out of view reads the next one straight away.
    observer.unobserve(sentinel);
    observer.observe(sentinel);
  };

  const paint = (state) => {
    const grew = painter.paint(content, state, state.complete);
    sentinel.hidden = state.complete;
    sentinel.textContent = `Showing ${humanBytes(state.end)} of ${humanBytes(state.total ?? file.size)}`;
    // Only pages that never said what the whole weighs are a cut with no end
    // in sight; any other incomplete body is the sentinel's to say.
    notice.hidden = !(state.pages.length && state.total === null);
    if (grew !== null) parked = grew === false;
    parked &&= !state.complete;
    if (state.complete) observer?.disconnect();
    else if (!parked) watchSentinel();
    onPaint();
  };

  const body = createPagedBody({ head, of: file.of, readPage, isCurrent, onChange: paint, onMoved: () => restart() });
  void body.hydrate().then((state) => {
    if (!disposed && !state.pages.length) restart();
  });

  return {
    more: () => body.more(),
    dispose() {
      disposed = true;
      observer?.disconnect();
      scroller.removeEventListener("scroll", readAlongTheLine, true);
      sentinel.removeEventListener("click", reachedEnd);
      body.dispose();
    },
  };
}
