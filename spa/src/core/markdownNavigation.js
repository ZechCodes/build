// Footnotes are local document links. Letting their fragment reach the SPA's
// hash router would replace the current route, so delegate their activation
// once, including keyboard-generated clicks, to scrolling and focus instead.
const FOOTNOTE_HREF = /^#md-fn(?:ref)?:[\da-f-]*:\d+(?::\d+)?$/;

function nearestTarget(anchor, id, root) {
  for (let parent = anchor.parentElement; parent; parent = parent.parentElement) {
    const target = [...parent.querySelectorAll("[id]")].find((element) => element.id === id);
    if (target) return target;
    if (parent === root) break;
  }
  return null;
}

function focusTarget(target) {
  if (!target.hasAttribute("tabindex") && target.tagName !== "A") {
    target.setAttribute("tabindex", "-1");
    target.addEventListener("blur", () => target.removeAttribute("tabindex"), { once: true });
  }
  target.scrollIntoView?.({ block: "nearest" });
  target.focus({ preventScroll: true });
}

export function installMarkdownNavigation(root = document) {
  const onClick = (event) => {
    const anchor = event.target?.closest?.(".md-footnote-ref > a, a.md-footnote-backref");
    if (!anchor || !FOOTNOTE_HREF.test(anchor.getAttribute("href") || "")) return;
    event.preventDefault();
    const target = nearestTarget(anchor, anchor.getAttribute("href").slice(1), root);
    if (target) focusTarget(target);
  };
  root.addEventListener("click", onClick);
  return () => root.removeEventListener("click", onClick);
}
