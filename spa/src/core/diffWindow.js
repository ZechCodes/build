import { fileKey } from "./diff.js";

export const CAPPED_PREVIEW_ROWS = 24;
export const COLLAPSED_PREVIEW_ROWS = 8;
export const ROW_WINDOW_SIZE = 240;
export const DIFF_ROW_HEIGHT = 20;
export const ROW_WINDOW_STRIDE = 120;

const clamp = (value, low, high) => Math.max(low, Math.min(value, high));

export function boundedRows(rows, fold) {
  const limit = fold === "shut" ? COLLAPSED_PREVIEW_ROWS : CAPPED_PREVIEW_ROWS;
  return { start: 0, end: Math.min(rows.length, limit) };
}

export function normalizeRowWindow(rowCount, requested, size = ROW_WINDOW_SIZE) {
  if (rowCount <= size) return { start: 0, end: rowCount };
  const start = clamp(Number(requested?.start) || 0, 0, Math.max(0, rowCount - size));
  const requestedEnd = Number(requested?.end) || 0;
  return { start, end: Math.min(rowCount, Math.max(start + size, requestedEnd)) };
}

/** The coarse window boundary for an inner diff scroll position. Keeping the
 * window fixed between boundaries lets native scrolling do the common work;
 * the keyed renderer only runs after half a window has passed. */
export function rowWindowStart(scrollTop, rowWindowSize = ROW_WINDOW_SIZE) {
  const stride = Math.max(1, Math.min(ROW_WINDOW_STRIDE, Math.floor(rowWindowSize / 2)));
  const bufferedRow = Math.max(0, Math.floor(scrollTop / DIFF_ROW_HEIGHT) - Math.floor(rowWindowSize / 4));
  return Math.floor(bufferedRow / stride) * stride;
}

export function rowWindowFor(file, fold, viewport) {
  if (fold !== "open") return boundedRows(file.rows, fold);
  if (!viewport) return { start: 0, end: file.rows.length };
  return normalizeRowWindow(file.rows.length, viewport?.rowWindow?.(fileKey(file), file.rows.length));
}

export function fileBodyIsVisible(file, fold, viewport) {
  if (!viewport) return true;
  return viewport.fileVisible ? viewport.fileVisible(fileKey(file)) : true;
}
