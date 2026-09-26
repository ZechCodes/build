// The Files tab's open files, as a strip of tabs over the preview. The layout
// ({tabs, active}) is plain UI state: which files are open and which one the
// preview shows. It says nothing about a file's body or its unsaved edits —
// those stay with the view that holds them, and are handed in only as the set
// of paths to mark.

import { esc } from "./text.js";

const fileNameOf = (path) => path.split("/").at(-1);

/** Pure: open `path` — a new tab at the end, or the tab it already has — and
 *  make it the active one. */
export function openTab(layout, path) {
  const tabs = layout.tabs.includes(path) ? layout.tabs : [...layout.tabs, path];
  return { tabs, active: path };
}

/** Pure: show an open tab. A path that is not open changes nothing. */
export function activateTab(layout, path) {
  return layout.tabs.includes(path) ? { tabs: layout.tabs, active: path } : layout;
}

/** Pure: close `path`. Closing the active tab shows its neighbour — the one
 *  that slides into its place, else the one before it — or nothing at all. */
export function closeTab(layout, path) {
  const index = layout.tabs.indexOf(path);
  const tabs = layout.tabs.filter((tab) => tab !== path);
  if (layout.active !== path) return { tabs, active: layout.active };
  return { tabs, active: tabs[Math.min(index, tabs.length - 1)] ?? null };
}

/** Pure: a remembered layout, read defensively — it is UI state from the cache
 *  and may be from another build. Non-paths, repeats and whatever `accepts`
 *  refuses (a file under a directory the workspace no longer has) are dropped,
 *  and an active tab that is not open falls back to the first one. */
export function readTabLayout(value, accepts = () => true) {
  const listed = Array.isArray(value?.tabs) ? value.tabs : [];
  const tabs = [...new Set(listed.filter((tab) => typeof tab === "string" && tab && accepts(tab)))];
  const active = tabs.includes(value?.active) ? value.active : tabs[0] ?? null;
  return { tabs, active };
}

/** Pure: what each open tab is called and titled. A tab is named by its file,
 *  and titled by its path; under a workspace's roots (#174), the title says the
 *  root first, and when two open files share a name each says its root before
 *  it — `root / name`. `locate(tab)` answers { root: { label }, path }; a
 *  single checkout's root has no label and its tabs read as they always have. */
export function tabLabels(tabs, locate) {
  const located = new Map(tabs.map((tab) => [tab, locate(tab)]));
  const names = [...located.values()].map(({ path }) => fileNameOf(path));
  const shared = new Set(names.filter((name, index) => names.indexOf(name) !== index));
  const rooted = (root, text) => (root.label ? `${root.label} / ${text}` : text);
  return new Map(
    [...located].map(([tab, { root, path }]) => {
      const name = fileNameOf(path);
      return [tab, { name: shared.has(name) ? rooted(root, name) : name, title: rooted(root, path) }];
    }),
  );
}

const pathLabels = (tabs) => tabLabels(tabs, (path) => ({ root: {}, path }));

const tabHtml = (active, dirty, labels) => (path) => {
  const label = labels.get(path);
  const name = esc(label.name);
  const selected = path === active;
  const unsaved = dirty.has(path);
  return `<div class="ftab${selected ? " active" : ""}${unsaved ? " dirty" : ""}" role="presentation">` +
    `<button type="button" class="ftab-name mono" role="tab" data-tab-path="${esc(path)}" title="${esc(label.title)}" aria-selected="${selected}">${name}</button>` +
    `<button type="button" class="ftab-close" data-tab-close="${esc(path)}" aria-label="Close ${name}${unsaved ? " (unsaved)" : ""}"><span class="ftab-mark" aria-hidden="true"></span></button>` +
    `</div>`;
};

/** Pure: the tab strip's HTML — one tab per open file, named and titled by
 *  `labels` (tabLabels; by the file and its path when none are given), each
 *  with a close control; a tab in `dirty` is marked unsaved. Paths are
 *  repo-derived and escaped everywhere they land. */
export function fileTabsHtml(layout, dirty, labels = pathLabels(layout.tabs)) {
  if (!layout.tabs.length) return "";
  return layout.tabs.map(tabHtml(layout.active, dirty, labels)).join("");
}
