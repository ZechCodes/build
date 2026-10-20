// A table is a keyboard stop only when there is horizontal content to reach.
// Rendering stays pure; this document-level controller measures cached paints
// and reacts to layout changes without retaining tables that left the DOM.
function updateFocus(wrapper, root) {
  if (!wrapper || !root.contains(wrapper)) return;
  if (wrapper.scrollWidth > wrapper.clientWidth) wrapper.setAttribute("tabindex", "0");
  else wrapper.removeAttribute("tabindex");
}

export function installMarkdownTableFocus(root = document) {
  const tracked = new Map();
  let disposed = false;
  let queued = false;
  const resize = new ResizeObserver((entries) => {
    if (disposed) return;
    for (const { target } of entries) updateFocus(target.closest(".mdtable"), root);
  });
  const release = (wrapper, table) => {
    resize.unobserve(wrapper);
    if (table) resize.unobserve(table);
    wrapper.removeAttribute("tabindex");
    tracked.delete(wrapper);
  };
  const track = (wrapper) => {
    const table = wrapper.querySelector("table");
    if (!tracked.has(wrapper) || tracked.get(wrapper) !== table) {
      if (tracked.has(wrapper)) release(wrapper, tracked.get(wrapper));
      tracked.set(wrapper, table);
      resize.observe(wrapper);
      if (table) resize.observe(table);
    }
    updateFocus(wrapper, root);
  };
  const refresh = () => {
    if (disposed) return;
    const current = new Set(root.querySelectorAll(".mdtable"));
    for (const [wrapper, table] of tracked) if (!current.has(wrapper)) release(wrapper, table);
    current.forEach(track);
  };
  const mutations = new MutationObserver(() => {
    if (queued || disposed) return;
    queued = true;
    queueMicrotask(() => { queued = false; refresh(); });
  });
  mutations.observe(root, { childList: true, subtree: true, characterData: true });
  refresh();
  return () => {
    if (disposed) return;
    disposed = true;
    mutations.disconnect();
    resize.disconnect();
    for (const wrapper of tracked.keys()) wrapper.removeAttribute("tabindex");
    tracked.clear();
  };
}
