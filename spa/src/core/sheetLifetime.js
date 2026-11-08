/** Release one opening's readers when its owner is hidden, removed or replaced.
 * Repaints can replace children: the opening supplies its current frame check.
 * Explicit buttons use the same idempotent teardown as external dismissal. */
export function watchSheetLifetime(current, release) {
  let active = true;
  const dispose = () => {
    if (!active) return;
    active = false;
    observer.disconnect();
    release();
  };
  const observer = new MutationObserver(() => {
    if (!current()) dispose();
  });
  observer.observe(document.documentElement, {
    childList: true, subtree: true, attributes: true, attributeFilter: ["class"],
  });
  return dispose;
}
