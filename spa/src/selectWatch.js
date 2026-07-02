// Pointer-agnostic text-selection watcher for the comment flows (plan + diff).
//
// Mouse selections end with a `pointerup` inside the container, but touch
// selections are made with native drag handles: the browser fires only
// `selectionchange` (repeatedly, while the handles move) and no pointer event
// lands on the text. Watching both — with a debounce so the popover appears
// once the handles settle — makes select-to-comment work on phones.

/**
 * Invoke `onSelect(selection)` whenever a non-empty selection settles inside
 * `container`. Returns a dispose function; callers must dispose the previous
 * watcher when re-rendering, or document-level listeners accumulate.
 */
export function watchSelection(container, onSelect) {
  let timer = null;
  const consider = () => {
    const sel = window.getSelection();
    if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return;
    if (!sel.toString().trim()) return;
    if (!container.isConnected) return;
    if (!container.contains(sel.anchorNode) || !container.contains(sel.focusNode)) return;
    onSelect(sel);
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
