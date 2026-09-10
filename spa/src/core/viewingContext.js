import { pathOf } from "./diff.js";
import { esc } from "./text.js";

const encoder = new TextEncoder();
const DEFAULT_LIMITS = Object.freeze({ maxItems: 100, maxSelectionBytes: 32 * 1024, maxPathBytes: 4 * 1024 });
const ITEM_KINDS = new Set(["file", "commit", "diff", "selection"]);

const byteLength = (value) => encoder.encode(value).length;

function truncateUtf8(value, limit) {
  const text = String(value || "");
  if (byteLength(text) <= limit) return { text, truncated: false };
  let clipped = "";
  let used = 0;
  for (const character of text) {
    const size = byteLength(character);
    if (used + size > limit) break;
    clipped += character;
    used += size;
  }
  return { text: clipped, truncated: true };
}

function validPath(path, limit) {
  const value = typeof path === "string" ? path : "";
  return value && byteLength(value) <= limit ? value : null;
}

const normalizeCommit = (item) => {
  const sha = String(item.sha || "");
  return /^[a-f\d]{40}(?:[a-f\d]{24})?$/i.test(sha) ? { kind: "commit", sha } : null;
};
const normalizeFile = (item, limits) => {
  const path = validPath(item.path, limits.maxPathBytes);
  if (!path) return null;
  if (item.kind === "file") return { kind: "file", path };
  if (item.kind === "diff") {
    return ["uncommitted", "all"].includes(item.mode) ? { kind: "diff", path, mode: item.mode } : null;
  }
  return { path };
};
function normalizeSelection(item, limits, remainingSelectionBytes) {
  const base = normalizeFile(item, limits);
  if (!base) return null;
  const clipped = truncateUtf8(item.text, remainingSelectionBytes);
  if (!clipped.text) return null;
  const normalized = { kind: "selection", path: base.path, text: clipped.text };
  Object.assign(normalized, normalizeLines(item));
  if (["old", "new"].includes(item.side)) normalized.side = item.side;
  if (item.unsaved === true) normalized.unsaved = true;
  if (item.truncated === true || clipped.truncated) normalized.truncated = true;
  return normalized;
}
const normalizeLines = (item) => {
  if (!Number.isInteger(item.line_start) || item.line_start <= 0) return {};
  if (!Number.isInteger(item.line_end) || item.line_end < item.line_start) return { line_start: item.line_start };
  return { line_start: item.line_start, line_end: item.line_end };
};
const NORMALIZERS = {
  commit: (item) => normalizeCommit(item),
  file: (item, limits) => normalizeFile(item, limits),
  diff: (item, limits) => normalizeFile(item, limits),
  selection: normalizeSelection,
};
const normalizeItem = (item, limits, remaining) =>
  item && ITEM_KINDS.has(item.kind) ? NORMALIZERS[item.kind](item, limits, remaining) : null;

export function normalizeViewingContext(items, options = {}) {
  const limits = { ...DEFAULT_LIMITS, ...options };
  const normalized = [];
  let selectionBytes = 0;
  for (const item of items || []) {
    if (normalized.length >= limits.maxItems) break;
    const next = normalizeItem(item, limits, Math.max(0, limits.maxSelectionBytes - selectionBytes));
    if (!next) continue;
    normalized.push(next);
    if (next.kind === "selection") selectionBytes += byteLength(next.text);
  }
  return normalized.length ? { version: 1, items: normalized } : undefined;
}

const intersectRect = (first, second) => ({
  top: Math.max(first.top, second.top), bottom: Math.min(first.bottom, second.bottom),
  left: Math.max(first.left, second.left), right: Math.min(first.right, second.right),
});

const viewportBounds = (view) => {
  const visual = view && view.visualViewport;
  return visual
    ? { top: visual.offsetTop, left: visual.offsetLeft, bottom: visual.offsetTop + visual.height, right: visual.offsetLeft + visual.width }
    : { top: 0, left: 0, bottom: view ? view.innerHeight : Infinity, right: view ? view.innerWidth : Infinity };
};
const clipsContents = (style) => style && /(auto|scroll|hidden|clip)/.test(`${style.overflow}${style.overflowX}${style.overflowY}`);

function clippedBounds(root) {
  const view = root.ownerDocument.defaultView;
  let bounds = intersectRect(root.getBoundingClientRect(), viewportBounds(view));
  for (let ancestor = root.parentElement; ancestor; ancestor = ancestor.parentElement) {
    const style = view ? view.getComputedStyle(ancestor) : null;
    if (clipsContents(style)) {
      bounds = intersectRect(bounds, ancestor.getBoundingClientRect());
    }
  }
  return bounds;
}

const elementIntersects = (element, rootRect) => {
  if (rootRect.bottom <= rootRect.top || rootRect.right <= rootRect.left) return false;
  const rect = element.getBoundingClientRect();
  const view = element.ownerDocument.defaultView;
  const style = view ? view.getComputedStyle(element) : null;
  const shown = !style || (style.display !== "none" && style.visibility !== "hidden");
  const verticallyVisible = rect.bottom > rootRect.top && rect.top < rootRect.bottom;
  const horizontallyVisible = rect.right > rootRect.left && rect.left < rootRect.right;
  return shown && verticallyVisible && horizontallyVisible;
};

function selectedRows(root, selection) {
  const range = selection.getRangeAt(0);
  return [...root.querySelectorAll(".file tr[data-side]")].filter((row) => {
    try { return range.intersectsNode(row); } catch { return false; }
  });
}

const appendSelectionRow = (groups, { path, side, line, text }) => {
  const previous = groups.at(-1);
  if (previous && previous.path === path && previous.side === side) {
    previous.text += `\n${text}`;
    previous.line_end = line;
  } else groups.push({ kind: "selection", path, text, line_start: line, line_end: line, side });
};

function selectionItems(root, selection) {
  const groups = [];
  for (const row of selectedRows(root, selection)) {
    const path = pathOf(row.closest(".file")?.dataset.key || "");
    const side = row.dataset.side;
    const line = Number(row.dataset[side === "old" ? "oldLine" : "newLine"]);
    const code = row.querySelector(".code");
    const text = code ? selectedText(selection.getRangeAt(0), code) : "";
    if (!path || !text) continue;
    appendSelectionRow(groups, { path, side, line, text });
  }
  return groups;
}

function selectedText(source, node) {
  const range = node.ownerDocument.createRange();
  const RangeType = node.ownerDocument.defaultView.Range;
  range.selectNodeContents(node);
  if (source.compareBoundaryPoints(RangeType.START_TO_START, range) > 0) range.setStart(source.startContainer, source.startOffset);
  if (source.compareBoundaryPoints(RangeType.END_TO_END, range) < 0) range.setEnd(source.endContainer, source.endOffset);
  return range.toString();
}

export function createViewingContext(options = {}) {
  let enabled = options.enabled !== false;
  let artifact = [];
  let selection = [];
  const dismissed = new Set();
  const itemIdentity = (item) => JSON.stringify(item);
  const listeners = new Set();
  let announced = "";
  const announce = () => {
    const snapshot = api.snapshot();
    const fingerprint = JSON.stringify(snapshot || null);
    if (fingerprint === announced) return;
    announced = fingerprint;
    listeners.forEach((listener) => listener(snapshot));
  };
  const api = {
    setEnabled(wanted) { enabled = Boolean(wanted); if (!enabled) api.clear(); },
    set(context) {
      const supplied = context?.version === 1 ? context.items || [] : context ? [context] : [];
      artifact = supplied.filter((item) => item.kind !== "selection");
      selection = supplied.filter((item) => item.kind === "selection");
      dismissed.clear();
      announce();
    },
    clear() { artifact = []; selection = []; dismissed.clear(); announce(); },
    setSelection(items) {
      selection = [...(items || [])];
      selection.forEach((item) => dismissed.delete(itemIdentity(item)));
      announce();
    },
    clearSelection() { selection = []; announce(); },
    clearSelectionIfMatches(context) {
      const sent = (context?.items || []).filter((item) => item.kind === "selection");
      const current = (api.snapshot()?.items || []).filter((item) => item.kind === "selection");
      if (sent.length && JSON.stringify(sent) === JSON.stringify(current)) api.clearSelection();
    },
    remove(index) {
      const joined = [...artifact, ...selection];
      const removed = joined.filter((item) => !dismissed.has(itemIdentity(item)))[index];
      if (removed) dismissed.add(itemIdentity(removed));
      announce();
    },
    setVisibleDiffs(scroller, mode) {
      if (!scroller) return api.clear();
      const rootRect = clippedBounds(scroller);
      artifact = [...scroller.querySelectorAll(".file[data-key]")]
        .filter((file) => elementIntersects(file, rootRect))
        .map((file) => ({ kind: "diff", path: pathOf(file.dataset.key), mode }));
      announce();
    },
    captureDomSelection(root) {
      const current = root?.ownerDocument?.getSelection?.();
      selection = current && !current.isCollapsed ? selectionItems(root, current) : [];
      announce();
    },
    snapshot() {
      if (!enabled) return undefined;
      const value = normalizeViewingContext([...artifact, ...selection].filter((item) => !dismissed.has(itemIdentity(item))), options);
      return value ? structuredClone(value) : undefined;
    },
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
  };
  return api;
}

const chipLabel = (item) => {
  if (item.kind === "commit") return `Commit ${item.sha.slice(0, 12)}`;
  if (item.kind === "selection") return `${item.path}${item.line_start ? `:${item.line_start}` : ""}`;
  if (item.kind === "diff") return `${item.mode === "all" ? "All changes" : "Uncommitted"}: ${item.path}`;
  return `File: ${item.path}`;
};

const selectionChip = (item, index, removable) => `<details class="viewing-context-detail" data-context-index="${index}">
  <summary class="viewing-context-chip selection">${esc(chipLabel(item))}${item.unsaved ? " · Unsaved" : ""}${item.truncated ? " · Truncated" : ""}</summary>
  <pre>${esc(item.text)}</pre>${removable ? '<button type="button" aria-label="Remove context">Remove</button>' : ""}
</details>`;

export function viewingContextChipsHtml(context, { removable = false } = {}) {
  return (context?.items || []).map((item, index) => item.kind === "selection" ? selectionChip(item, index, removable) :
    `<span class="viewing-context-chip ${esc(item.kind)}" data-context-index="${index}" title="${esc(chipLabel(item))}">${esc(chipLabel(item))}${removable ? `<button type="button" aria-label="Remove context">×</button>` : ""}</span>`,
  ).join("");
}
