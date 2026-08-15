// Pointer-agnostic text-selection watcher for the comment flows (plan + diff).
//
// Mouse selections end with a `pointerup` inside the container, but touch
// selections are made with native drag handles: the browser fires only
// `selectionchange` (repeatedly, while the handles move) and no pointer event
// lands on the text. Watching both — with a debounce so the popover appears
// once the handles settle — makes select-to-comment work on phones.

/**
 * The live, non-empty selection inside `container`, or null. This is a reading
 * of RIGHT NOW, not of a settled selection: a controller asks it to find out
 * whether someone is in the middle of dragging text out of the subtree it is
 * about to rebuild — a rebuild there cancels the selection, and the watcher
 * below has not called back about it yet.
 */
export function selectionInside(container) {
  if (!container || !container.isConnected) return null;
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return null;
  if (!selection.toString().trim()) return null;
  if (!container.contains(selection.anchorNode) || !container.contains(selection.focusNode)) return null;
  return selection;
}

/**
 * Invoke `onSelect(selection)` whenever a non-empty selection settles inside
 * `container`. Returns a dispose function; callers must dispose the previous
 * watcher when re-rendering, or document-level listeners accumulate.
 */
export function watchSelection(container, onSelect) {
  let timer = null;
  const consider = () => {
    const selection = selectionInside(container);
    if (selection) onSelect(selection);
  };
  const onPointerUp = () => setTimeout(consider, 0);
  const onSelectionChange = () => {
    clearTimeout(timer);
    timer = setTimeout(consider, 350);
  };
  container.addEventListener("pointerup", onPointerUp);
  document.addEventListener("selectionchange", onSelectionChange);
  return () => {
    container.removeEventListener("pointerup", onPointerUp);
    document.removeEventListener("selectionchange", onSelectionChange);
    clearTimeout(timer);
  };
}
