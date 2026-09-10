function isInside(root, node) {
  return Boolean(root && node && (root === node || root.contains(node)));
}

function selectionRange(root, selection) {
  if (!root || !selection || selection.isCollapsed || selection.rangeCount < 1) return null;
  if (!isInside(root, selection.anchorNode) || !isInside(root, selection.focusNode)) return null;
  try {
    return selection.getRangeAt(0);
  } catch {
    return null;
  }
}

function closestWithin(root, node, selector) {
  const element = node?.nodeType === 1 ? node : node?.parentElement;
  const match = element?.closest?.(selector);
  return match && isInside(root, match) ? match : null;
}

function elementIsHidden(element, view) {
  if (element.hidden || element.getAttribute("aria-hidden") === "true") return true;
  const style = view?.getComputedStyle?.(element);
  return style?.display === "none" || style?.visibility === "hidden";
}

function isHidden(node, boundary) {
  const view = node.ownerDocument.defaultView;
  for (let element = node.parentElement; element && element !== boundary; element = element.parentElement) {
    if (elementIsHidden(element, view)) return true;
  }
  return false;
}

function clippedText(range, textNode) {
  let intersects;
  try {
    intersects = range.intersectsNode(textNode);
  } catch {
    return "";
  }
  if (!intersects) return "";
  const clipped = textNode.ownerDocument.createRange();
  const RangeType = textNode.ownerDocument.defaultView.Range;
  clipped.selectNodeContents(textNode);
  if (range.compareBoundaryPoints(RangeType.START_TO_START, clipped) > 0) {
    clipped.setStart(range.startContainer, range.startOffset);
  }
  if (range.compareBoundaryPoints(RangeType.END_TO_END, clipped) < 0) {
    clipped.setEnd(range.endContainer, range.endOffset);
  }
  return clipped.toString();
}

function visibleSelectedText(range, container) {
  const view = container.ownerDocument.defaultView;
  const walker = container.ownerDocument.createTreeWalker(container, view.NodeFilter.SHOW_TEXT);
  const fragments = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!isHidden(node, container)) fragments.push(clippedText(range, node));
  }
  return fragments.join("");
}

function rangeIntersects(range, node) {
  try {
    return range.intersectsNode(node);
  } catch {
    return false;
  }
}

function sourceSelection(source, path, range) {
  const rows = [...source.querySelectorAll("tr[data-new-line]")];
  if (!rows.length) {
    const text = visibleSelectedText(range, source);
    return text ? [{ kind: "selection", path, text }] : [];
  }
  const lines = [];
  for (const row of rows) {
    if (!rangeIntersects(range, row)) continue;
    const code = row.querySelector("td.fsrc-code");
    if (!code || !rangeIntersects(range, code)) continue;
    const text = visibleSelectedText(range, code);
    lines.push({ line: Number(row.dataset.newLine), text });
  }
  if (!lines.length) return [];
  return [{
    kind: "selection",
    path,
    text: lines.map(({ text }) => text).join("\n"),
    line_start: lines[0].line,
    line_end: lines.at(-1).line,
  }];
}

function markdownSelection(plan, path, range) {
  const text = range.toString();
  return text ? [{ kind: "selection", path, text }] : [];
}

const validPath = (path) => typeof path === "string" && path.length > 0;
const documentSelection = (root) => root ? root.ownerDocument.getSelection() : null;

/** Capture the visible browser selection from a Files source or Markdown view. */
export function captureFileSelection(root, path, selection = documentSelection(root)) {
  if (!validPath(path)) return [];
  const range = selectionRange(root, selection);
  if (!range) return [];
  const source = closestWithin(root, range.commonAncestorContainer, ".fsrc");
  if (source) return sourceSelection(source, path, range);
  const plan = closestWithin(root, range.commonAncestorContainer, ".plan");
  if (plan) return markdownSelection(plan, path, range);
  return [];
}
