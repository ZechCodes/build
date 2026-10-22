// A table is a keyboard stop only when there is horizontal content to reach.
// Rendering stays pure; this document-level controller measures cached paints
// and reacts to layout changes without retaining tables that left the DOM.
function updateFocus(wrapper, root) {
  if (!wrapper || !root.contains(wrapper)) return;
  if (wrapper.scrollWidth > wrapper.clientWidth) wrapper.setAttribute("tabindex", "0");
  else wrapper.removeAttribute("tabindex");
}

function elementsIn(node, selector) {
  if (node.nodeType !== 1) return [];
  return [...(node.matches(selector) ? [node] : []), ...node.querySelectorAll(selector)];
}

function addedWrappers(node, changed) {
  for (const table of elementsIn(node, "table")) {
    const wrapper = table.closest(".mdtable");
    if (wrapper) changed.add(wrapper);
  }
}

function removedWrappers(node, changed, owners) {
  for (const wrapper of elementsIn(node, ".mdtable")) changed.add(wrapper);
  for (const table of elementsIn(node, "table")) {
    const wrapper = owners.get(table);
    if (wrapper) changed.add(wrapper);
  }
}

export function installMarkdownTableFocus(root = document) {
  const tracked = new Map();
  const owners = new WeakMap();
  let disposed = false;
  const resize = new ResizeObserver((entries) => {
    if (disposed) return;
    const changed = new Set(entries.map(({ target }) => target.closest(".mdtable")));
    for (const wrapper of changed) if (tracked.has(wrapper)) updateFocus(wrapper, root);
  });
  const release = (wrapper, table) => {
    resize.unobserve(wrapper);
    // A table may already belong to a newly painted wrapper in this batch.
    if (owners.get(table) === wrapper) {
      resize.unobserve(table);
      owners.delete(table);
    }
    wrapper.removeAttribute("tabindex");
    tracked.delete(wrapper);
  };
  const track = (wrapper) => {
    const previous = tracked.get(wrapper);
    if (!root.contains(wrapper)) {
      if (previous) release(wrapper, previous);
      return;
    }
    const table = wrapper.querySelector("table");
    if (previous === table) return;
    if (previous) release(wrapper, previous);
    if (!table) return;
    tracked.set(wrapper, table);
    owners.set(table, wrapper);
    resize.observe(wrapper);
    resize.observe(table);
    updateFocus(wrapper, root);
  };
  const mutations = new MutationObserver((records) => {
    if (disposed) return;
    const changed = new Set();
    for (const record of records) {
      for (const node of record.removedNodes) removedWrappers(node, changed, owners);
      for (const node of record.addedNodes) addedWrappers(node, changed);
    }
    changed.forEach(track);
  });
  // Text streaming and timer updates cannot introduce tables. Actual size
  // changes are handled by ResizeObserver; only table ownership changes here.
  mutations.observe(root, { childList: true, subtree: true });
  root.querySelectorAll(".mdtable").forEach(track);
  return () => {
    if (disposed) return;
    disposed = true;
    mutations.disconnect();
    resize.disconnect();
    for (const wrapper of tracked.keys()) wrapper.removeAttribute("tabindex");
    tracked.clear();
  };
}
