import { esc } from "./text.js";

/** Where an arrow takes the selection along the row. */
const KEY_STEPS = { ArrowLeft: -1, ArrowRight: 1 };
const KEY_ENDS = { Home: 0, End: -1 };

/** Pure: the tab row — one tab per directory, in the workspace's order, git or
 *  not, the current one selected and the one Tab lands on. Names come from the
 *  user and the repos, so they are escaped. */
export function directoryTabsHtml(directories) {
  const tabs = directories
    .map(
      (directory) =>
        `<button class="workspace-dirtab${directory.current ? " current" : ""}" type="button" role="tab" data-directory="${esc(directory.sourceId)}" aria-selected="${directory.current}" tabindex="${directory.current ? 0 : -1}">${esc(directory.label)}</button>`,
    )
    .join("");
  return `<div class="workspace-dirtabs" role="tablist" aria-label="Workspace directories">${tabs}</div>`;
}

export function wireDirectoryTabs(row, onSelect) {
  const cells = () => [...row.querySelectorAll("[data-directory]")];
  row.onclick = (event) => {
    const cell = event.target.closest("[data-directory]");
    if (cell) onSelect(cell.dataset.directory);
  };
  row.onkeydown = (event) => {
    const step = KEY_STEPS[event.key];
    const end = KEY_ENDS[event.key];
    const all = cells();
    const from = all.indexOf(event.target.closest("[data-directory]"));
    if ((step === undefined && end === undefined) || from < 0) return;
    event.preventDefault();
    const to = step === undefined ? all.at(end) : all[(from + step + all.length) % all.length];
    to.focus();
    onSelect(to.dataset.directory);
  };
}

