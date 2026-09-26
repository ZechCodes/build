// A file too large for one cache record, painted from its pages (#95).
//
// The file's own record says what the file is; its bytes are page records
// beside it (core/bodyPages.js). This paints what the cache holds, never an
// answer. Source is painted a page at a time: each page's lines are appended
// under the rows already painted, numbered on from them, and a sentinel under
// the last row says how much of the file that is. When the sentinel scrolls
// into view the next page is read into the cache, and the cache's
// announcement paints it. A body the viewer can only show whole (a picture, a
// film, a rendered HTML page) is painted once every page is held.
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

/** The lines of a body, a page at a time. A bridge cuts its pages after a
 *  line end, but a line longer than a page is cut inside it, and a body split
 *  here (read whole from an older bridge) is cut anywhere, even inside a
 *  character. So the bytes are decoded as one stream, and a line is given out
 *  once its end has arrived, or once no more of the body will. */
function createLineReader() {
  const decoder = new TextDecoder();
  let carry = "";
  return {
    take(page) {
      const lines = (carry + decoder.decode(bytesOfBase64(page.body), { stream: true })).split("\n");
      carry = lines.pop();
      return lines;
    },
    finish() {
      const last = carry + decoder.decode();
      carry = "";
      return [last];
    },
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
 *  the ones it shows (evicted, or of another version). */
export function sourceLinesPainter(lang) {
  let painted = null;
  const start = (content) => {
    content.innerHTML = `<div class="fsrc"><table></table></div>`;
    painted = { table: content.querySelector("table"), pages: 0, end: 0, lines: 0, finished: false, reader: createLineReader() };
  };
  const append = (lines) => {
    if (!lines.length) return;
    const rows = painted.table.ownerDocument.createElement("tbody");
    rows.innerHTML = sourceRowsHtml(lines, lang, painted.lines + 1);
    painted.table.append(rows);
    painted.lines += lines.length;
  };
  return {
    paint(content, state, final) {
      if (!painted || !paintedFollows(painted, state)) start(content);
      for (const page of state.pages.slice(painted.pages)) append(painted.reader.take(page));
      painted.pages = state.pages.length;
      painted.end = state.end;
      if (final && !painted.finished) {
        append(painted.reader.finish());
        painted.finished = true;
      }
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
    painter.paint(content, state, state.complete);
    sentinel.hidden = state.complete;
    sentinel.textContent = `Showing ${humanBytes(state.end)} of ${humanBytes(state.total ?? file.size)}`;
    // Only pages that never said what the whole weighs are a cut with no end
    // in sight; any other incomplete body is the sentinel's to say.
    notice.hidden = !(state.pages.length && state.total === null);
    if (state.complete) observer?.disconnect();
    else watchSentinel();
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
      body.dispose();
    },
  };
}
