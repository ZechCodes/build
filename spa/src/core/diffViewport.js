import { DIFF_ROW_HEIGHT, ROW_WINDOW_SIZE, rowWindowStart } from "./diffWindow.js";
const FILE_ELEMENT = ".file[data-key]";

const FILE_MARGIN = "1200px 0px";

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
export function createDiffViewport({ repaint, commentLayerBusy = () => false, rowWindowSize = ROW_WINDOW_SIZE } = {}) {
  const visible = new Set();
  const requested = new Set();
  const initialLoads = new Set();
  const windows = new Map();
  let scroller = null;
  let observer = null;
  let frame = 0;
  let pending = false;
  let preservedSelection = null;

  const page = () => scroller?.ownerDocument || globalThis.document;
  const view = () => page()?.defaultView || globalThis;
  const schedule = (callback) =>
    view().requestAnimationFrame ? view().requestAnimationFrame(callback) : view().setTimeout(callback, 0);
  const cancel = (handle) =>
    view().cancelAnimationFrame ? view().cancelAnimationFrame(handle) : view().clearTimeout(handle);

  const requestPaint = (force = false) => {
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
    // The composer is fixed under document.body, outside the diff scroller.
    // Pin the rendered rows while its real input is mounted so focusing it
    // cannot make the selection collapse and release the rows beneath it.
    const commentComposer = page()?.querySelector("body > .comment-pop .cp-input");
    return Boolean(selectionInside() || commentComposer || commentLayerBusy());
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

  const onScroll = (event) => {
    const box = event.target.closest?.(".dscroll[data-row-count]");
    if (!box) return;
    const count = Number(box.dataset.rowCount) || 0;
    const key = box.closest(FILE_ELEMENT)?.dataset.key;
    if (key && count > rowWindowSize) scrollUpdate(box, count, key);
  };

  const detachScroller = () => {
    observer?.disconnect();
    observer = null;
    scroller?.removeEventListener("scroll", onScroll, true);
    page()?.removeEventListener("selectionchange", onInteractionChange);
    page()?.removeEventListener("pointerup", onInteractionChange);
  };

  const attachScroller = () => {
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
    detachScroller();
    visible.clear();
    windows.clear();
    scroller = nextScroller;
    attachScroller();
  };

  return {
    attach(nextScroller) {
      replaceScroller(nextScroller);
      if (observer && visible.size === 0) {
        for (const file of [...scroller.querySelectorAll(FILE_ELEMENT)].slice(0, 3)) visible.add(file.dataset.key);
      }
      observeFiles();
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
    dispose() {
      if (frame) cancel(frame);
      frame = 0;
      detachScroller();
      scroller = null;
      visible.clear();
      windows.clear();
      requested.clear();
      initialLoads.clear();
    },
  };
}
