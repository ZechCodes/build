// Progressive enhancement of renderer-owned fences. The synchronous markdown
// entry point stays escape-first and the source remains available on failure.
const SELECTOR = ".md-mermaid";
const themeOf = (doc) => doc.documentElement.dataset.theme === "dark" ? "dark" : "light";
const sourceOf = (block) => block.querySelector(":scope > pre > code")?.textContent;

function showResult(block, svg) {
  block.querySelector(":scope > svg, :scope > .md-mermaid-error")?.remove();
  for (const source of block.querySelectorAll(":scope > pre, :scope > .md-code-label")) source.hidden = Boolean(svg);
  if (svg) block.prepend(svg);
  else {
    const error = block.ownerDocument.createElement("p");
    error.className = "md-mermaid-error";
    error.textContent = "Diagram unavailable";
    block.append(error);
  }
}

function blocksIn(node) {
  if (node.nodeType !== 1) return [];
  return [...(node.matches(SELECTOR) ? [node] : []), ...node.querySelectorAll(SELECTOR)];
}

export function installMarkdownMermaid(root = document, { load = () => import("./mermaidRenderer.js") } = {}) {
  const doc = root.ownerDocument || root;
  const states = new WeakMap();
  let disposed = false;
  const paint = (block) => {
    const source = sourceOf(block);
    const theme = themeOf(doc);
    if (source === undefined || !root.contains(block)) return;
    const key = `${theme}\n${source}`;
    if (states.get(block)?.key === key) return;
    const state = { key };
    states.set(block, state);
    const apply = (svg) => {
      if (disposed || states.get(block) !== state) return;
      if (!root.contains(block)) { states.delete(block); return; }
      // A source/theme change may precede its MutationObserver callback.
      if (sourceOf(block) !== source || themeOf(doc) !== theme) { states.delete(block); paint(block); return; }
      showResult(block, svg);
    };
    Promise.resolve().then(load).then(({ renderMermaid }) => renderMermaid(source, theme, doc)).then(apply, () => apply(null));
  };
  const changes = new MutationObserver((records) => {
    const blocks = new Set();
    for (const record of records) {
      const owner = record.target.parentElement?.closest(SELECTOR) || record.target.closest?.(SELECTOR);
      if (owner) blocks.add(owner);
      for (const node of record.addedNodes) blocksIn(node).forEach((block) => blocks.add(block));
    }
    blocks.forEach(paint);
  });
  changes.observe(root, { childList: true, subtree: true, characterData: true });
  const themes = new MutationObserver(() => root.querySelectorAll(SELECTOR).forEach(paint));
  themes.observe(doc.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  root.querySelectorAll(SELECTOR).forEach(paint);
  return () => { disposed = true; changes.disconnect(); themes.disconnect(); };
}
