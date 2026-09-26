import { DIFF_ROW_HEIGHT, ROW_WINDOW_SIZE, rowWindowStart } from "./diffWindow.js";
const FILE_ELEMENT = ".file[data-key]";

const FILE_MARGIN = "1200px 0px";

// Where a body kept in pages (#95) asks to be read on: a marker naming what to
// read more of, at the end of what is drawn of it. A marker inside a file is
// reached when that file's own row window scrolls near its end. One outside
// every file ends the stack; where it names the file its pages end inside
// (`data-more-file`), it is reached with that file's own rows — its window's
// end, or the stack's once every row of it is drawn; never while that file is
// not drawn at all, folded into a collapsed generated-files group — and
// otherwise when the stack's scroller reaches its end.
const MORE_MARKER = "[data-more-key]";
const MORE_MARGIN_PX = 60 * DIFF_ROW_HEIGHT;
const WINDOWED_BODY = ".dscroll.dwindow";

const nearItsEnd = (box) => box.scrollHeight - box.scrollTop - box.clientHeight <= MORE_MARGIN_PX;

/** The scroller whose end reaches `marker`: its file's windowed body, the
 *  stack's own scroller, or none for a file drawn capped, folded shut or not
 *  drawn at all, with rows the reader has not been shown. */
function scrollerReaching(marker, stackScroller) {
  const inside = marker.closest(FILE_ELEMENT);
  if (inside) return inside.querySelector(WINDOWED_BODY);
  const named = marker.dataset.moreFile;
  if (!named) return stackScroller;
  const file = [...stackScroller.querySelectorAll(FILE_ELEMENT)].find((candidate) => candidate.dataset.key === named);
  if (!file) return null;
  return file.querySelector(WINDOWED_BODY) || (everyRowDrawn(file) ? stackScroller : null);
}

/** Whether a file drawn without a window shows every row it has: a capped
 *  file's first rows, or a placeholder for one off screen, do not. */
function everyRowDrawn(file) {
  const box = file.querySelector("[data-row-count]");
  if (!box || box.classList.contains("dvirtual")) return false;
  return box.querySelectorAll("tr[aria-rowindex]").length >= Number(box.dataset.rowCount);
}

function nextScrollWindow({ held, start, selectionStart, selectionEnd, rowCount, rowWindowSize, scrollTop, selecting }) {
  const currentStart = held?.start ?? 0;
  const currentEnd = held?.end ?? Math.min(rowCount, currentStart + rowWindowSize);
  if (!selecting) {
    return { state: { start, scrollTop }, repaint: currentStart !== start, force: false };
  }
  const startBuffer = Math.floor(rowWindowSize / 2);
  const nextStart = Math.min(currentStart, selectionStart <= currentStart + 20 ? Math.max(0, selectionStart - startBuffer) : currentStart);
  const nextEnd = Math.max(currentEnd, selectionEnd >= currentEnd - 20 ? Math.min(rowCount, selectionEnd + startBuffer) : currentEnd);
  const repaint = nextStart !== currentStart || nextEnd !== currentEnd;
  return { state: { start: nextStart, end: nextEnd, scrollTop }, repaint, force: repaint };
}

function selectedPoint(page, scroller, node, offset) {
  const element = node?.nodeType === 1 ? node : node?.parentElement;
  const row = element?.closest?.("tr[aria-rowindex]");
  if (!row || !scroller.contains(row)) return null;
  const fileKey = row.closest(FILE_ELEMENT)?.dataset.key;
  if (!fileKey) return null;
  const range = page.createRange();
  range.selectNodeContents(row);
  range.setEnd(node, offset);
  return { fileKey, rowIndex: row.getAttribute("aria-rowindex"), offset: range.toString().length };
}

function selectedRow(scroller, point) {
  const file = [...scroller.querySelectorAll(FILE_ELEMENT)].find((candidate) => candidate.dataset.key === point.fileKey);
  return file?.querySelector(`tr[aria-rowindex="${point.rowIndex}"]`) || null;
}

function pointInRow(page, row, offset) {
  const walker = page.createTreeWalker(row, 4);
  let remaining = offset;
  let node = walker.nextNode();
  while (node && remaining > node.data.length) {
    remaining -= node.data.length;
    node = walker.nextNode();
  }
  return node ? { node, offset: Math.min(remaining, node.data.length) } : null;
}

/** Keeps the renderer's visibility and row-window decisions outside the DOM
 * markup. The controller only records geometry; the normal keyed repaint owns
 * all mutations. */
export function createDiffViewport({
  repaint,
  commentLayerBusy = () => false,
  rowWindowSize = ROW_WINDOW_SIZE,
  onNeedMore = () => {},
} = {}) {
  const visible = new Set();
  const requested = new Set();
  const initialLoads = new Set();
  const windows = new Map();
  let scroller = null;
  let observer = null;
  let frame = 0;
  let pending = false;
  let preservedSelection = null;
  let active = true;

  const page = () => scroller?.ownerDocument || globalThis.document;
  const view = () => page()?.defaultView || globalThis;
  const schedule = (callback) =>
    view().requestAnimationFrame ? view().requestAnimationFrame(callback) : view().setTimeout(callback, 0);
  const cancel = (handle) =>
    view().cancelAnimationFrame ? view().cancelAnimationFrame(handle) : view().clearTimeout(handle);

  const requestPaint = (force = false) => {
    if (!active) {
      pending = true;
      return;
    }
    if (frame) return;
    if (!force && frozen()) {
      pending = true;
      return;
    }
    pending = false;
    frame = schedule(() => {
      frame = 0;
      if (!force && frozen()) {
        pending = true;
        return;
      }
      if (force) preservedSelection = selectionSnapshot();
      repaint?.();
      restoreSelection();
    });
  };
  // Document-level interaction events only release viewport work that was
  // deferred while a selection or comment composer held the rendered rows.
  // Treating every pointerup as new work repaints the whole git pane for taps
  // anywhere in the app; on touch browsers that can replace the tapped node
  // between pointerup and the synthesized click, swallowing its activation.
  const onInteractionChange = () => {
    if (pending) requestPaint();
  };

  const selectionInside = () => {
    const selection = page()?.getSelection?.();
    return Boolean(selection && !selection.isCollapsed && scroller &&
      (scroller.contains(selection.anchorNode) || scroller.contains(selection.focusNode)));
  };

  const selectionSnapshot = () => {
    const selection = page()?.getSelection?.();
    if (!selection || selection.isCollapsed || !scroller) return null;
    const anchor = selectedPoint(page(), scroller, selection.anchorNode, selection.anchorOffset);
    const focus = selectedPoint(page(), scroller, selection.focusNode, selection.focusOffset);
    return anchor && focus ? { anchor, focus } : null;
  };

  const restoreSelection = () => {
    const snapshot = preservedSelection;
    preservedSelection = null;
    if (!snapshot || !scroller) return;
    const anchorRow = selectedRow(scroller, snapshot.anchor);
    const focusRow = selectedRow(scroller, snapshot.focus);
    const anchor = anchorRow && pointInRow(page(), anchorRow, snapshot.anchor.offset);
    const focus = focusRow && pointInRow(page(), focusRow, snapshot.focus.offset);
    if (anchor && focus) page().getSelection()?.setBaseAndExtent(anchor.node, anchor.offset, focus.node, focus.offset);
  };

  const frozen = () => {
    // The comment layer answers only for this viewport's composer. Another
    // directory may have its own draft open while this one is mounted hidden.
    return Boolean(selectionInside() || commentLayerBusy());
  };

  const observeFiles = () => {
    if (!observer || !scroller) return;
    for (const file of scroller.querySelectorAll(FILE_ELEMENT)) {
      observer.observe(file);
      const box = file.querySelector(".dscroll[data-row-count]");
      const held = windows.get(file.dataset.key);
      if (box && held?.scrollTop != null && box.scrollTop !== held.scrollTop) box.scrollTop = held.scrollTop;
    }
  };

  const onIntersect = (entries) => {
    if (!active) return;
    let changed = false;
    for (const entry of entries) {
      const key = entry.target.dataset.key;
      if (!key) continue;
      requested.delete(key);
      if (entry.isIntersecting && !visible.has(key)) {
        visible.add(key);
        changed = true;
      } else if (!entry.isIntersecting && visible.delete(key)) changed = true;
    }
    if (changed) requestPaint();
  };

  const scrollUpdate = (box, count, key) => {
    const held = windows.get(key);
    const start = rowWindowStart(box.scrollTop, rowWindowSize);
    const selectionStart = Math.floor(box.scrollTop / DIFF_ROW_HEIGHT);
    const selectionEnd = Math.floor((box.scrollTop + box.clientHeight) / DIFF_ROW_HEIGHT);
    const update = nextScrollWindow({
      held,
      start,
      selectionStart,
      selectionEnd,
      rowCount: count,
      rowWindowSize,
      scrollTop: box.scrollTop,
      selecting: selectionInside(),
    });
    windows.set(key, update.state);
    if (update.repaint) requestPaint(update.force);
  };

  /** Ask for more of every paged body whose drawn end the reader has
   *  reached. Only a scroll or a paint asks: a page landing asks nothing by
   *  itself, so an owner holding its paint still (a draft, an action in
   *  flight) is never read to the end behind the reader's back. */
  const askForMore = () => {
    if (!scroller) return;
    for (const marker of scroller.querySelectorAll(MORE_MARKER)) {
      const box = scrollerReaching(marker, scroller);
      if (box && nearItsEnd(box)) void onNeedMore(marker.dataset.moreKey);
    }
  };

  /** Ask after a paint, a frame on: the paint a landing page causes runs
   *  while that page's read is still the owner's one in flight, and asking
   *  inside it would be answered with that same read. */
  let askFrame = 0;
  const askAfterPaint = () => {
    if (askFrame || !scroller?.querySelector(MORE_MARKER)) return;
    askFrame = schedule(() => {
      askFrame = 0;
      askForMore();
    });
  };
  const cancelAsk = () => {
    if (askFrame) cancel(askFrame);
    askFrame = 0;
  };

  const onScroll = (event) => {
    const box = event.target.closest?.(".dscroll[data-row-count]");
    const count = Number(box?.dataset.rowCount) || 0;
    const key = box?.closest(FILE_ELEMENT)?.dataset.key;
    if (key && count > rowWindowSize) scrollUpdate(box, count, key);
    askForMore();
  };

  const detachScroller = () => {
    observer?.disconnect();
    observer = null;
    scroller?.removeEventListener("scroll", onScroll, true);
    page()?.removeEventListener("selectionchange", onInteractionChange);
    page()?.removeEventListener("pointerup", onInteractionChange);
  };

  const attachScroller = () => {
    if (!active) return;
    scroller?.addEventListener("scroll", onScroll, true);
    page()?.addEventListener("selectionchange", onInteractionChange);
    page()?.addEventListener("pointerup", onInteractionChange);
    if (scroller && globalThis.IntersectionObserver) {
      observer = new IntersectionObserver(onIntersect, { root: scroller, rootMargin: FILE_MARGIN });
    }
  };

  const replaceScroller = (nextScroller) => {
    if (scroller === nextScroller) return;
    if (frame) cancel(frame);
    frame = 0;
    cancelAsk();
    detachScroller();
    visible.clear();
    windows.clear();
    scroller = nextScroller;
    attachScroller();
  };

  return {
    attach(nextScroller) {
      const hadScroller = scroller !== null;
      replaceScroller(nextScroller);
      if (observer) {
        const files = [...scroller.querySelectorAll(FILE_ELEMENT)];
        const mountedKeys = new Set(files.map((file) => file.dataset.key));
        for (const key of visible) if (!mountedKeys.has(key)) visible.delete(key);
        if (visible.size === 0) {
          for (const file of files.slice(0, 3)) visible.add(file.dataset.key);
          if (hadScroller && visible.size > 0) requestPaint();
        }
      }
      observeFiles();
      askAfterPaint();
      if (pending) requestPaint();
    },
    renderOptions() {
      let seeded = 0;
      return {
        viewport: {
          fileVisible: (key) => requested.has(key) || (scroller ? !observer || visible.has(key) : seeded++ < 3),
          rowWindow: (key) => windows.get(key),
        },
      };
    },
    /** Whether row data is worth fetching. Explicit expansion wins until this
     * controller is disposed, including before IntersectionObserver reports. */
    shouldLoad(key, fold) {
      if (fold === "shut") return false;
      const normalized = String(key);
      if (requested.has(normalized) || (scroller && (!observer || visible.has(normalized)))) return true;
      if (scroller || initialLoads.size >= 3) return false;
      initialLoads.add(normalized);
      return true;
    },
    request(key) {
      requested.add(String(key));
      requestPaint();
    },
    setVisible(next) {
      if (active === next) return;
      active = next;
      if (!active) {
        if (frame) cancel(frame);
        frame = 0;
        preservedSelection = null;
        detachScroller();
      } else {
        attachScroller();
        observeFiles();
        if (pending) requestPaint();
      }
    },
    dispose() {
      if (frame) cancel(frame);
      frame = 0;
      cancelAsk();
      detachScroller();
      scroller = null;
      visible.clear();
      windows.clear();
      requested.clear();
      initialLoads.clear();
    },
  };
}
