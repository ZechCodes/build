export const DIFF_SORT_KEY = "build.diffSort";
export const DIFF_SORT_LATEST = "latest";
export const DIFF_SORT_ALPHABETICAL = "alphabetical";

export function loadDiffSort(storage) {
  try {
    const target = storage === undefined ? globalThis.localStorage : storage;
    return target?.getItem(DIFF_SORT_KEY) === DIFF_SORT_ALPHABETICAL
      ? DIFF_SORT_ALPHABETICAL
      : DIFF_SORT_LATEST;
  } catch {
    return DIFF_SORT_LATEST;
  }
}

export function saveDiffSort(order, storage) {
  try {
    const target = storage === undefined ? globalThis.localStorage : storage;
    target?.setItem(DIFF_SORT_KEY, order === DIFF_SORT_ALPHABETICAL ? DIFF_SORT_ALPHABETICAL : DIFF_SORT_LATEST);
  } catch {
    // A blocked or full local store must not make the Changes view unusable.
  }
}

const pathOrder = (left, right) => String(left.path || "").localeCompare(String(right.path || ""));

export function sortDiffFiles(files, order) {
  const sorted = [...files];
  if (order === DIFF_SORT_ALPHABETICAL) return sorted.sort(pathOrder);
  return sorted.sort((left, right) => {
    const leftTime = Number(left.editedAt) || 0;
    const rightTime = Number(right.editedAt) || 0;
    return rightTime - leftTime || pathOrder(left, right);
  });
}

export function diffSortHtml(order) {
  const selected = order === DIFF_SORT_ALPHABETICAL ? DIFF_SORT_ALPHABETICAL : DIFF_SORT_LATEST;
  return `<label class="diffsort"><span>Sort</span><select class="diffsort-select" aria-label="Sort changed files">
    <option value="latest"${selected === DIFF_SORT_LATEST ? " selected" : ""}>Latest changes</option>
    <option value="alphabetical"${selected === DIFF_SORT_ALPHABETICAL ? " selected" : ""}>Alphabetical</option>
  </select></label>`;
}
