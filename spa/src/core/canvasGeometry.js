// Animation reuses the CSS box until layout reports a size change. Reading a
// rectangle every frame can force layout after unrelated cache-driven paints.
function cssBoxOf(canvas) {
  const rect = typeof canvas.getBoundingClientRect === "function" ? canvas.getBoundingClientRect() : null;
  return {
    width: Math.max(0, rect?.width || canvas.clientWidth || 0),
    height: Math.max(0, rect?.height || canvas.clientHeight || 0),
  };
}

export function createCanvasGeometry(canvas, repaint) {
  let box = null;
  let disposed = false;
  const changed = () => {
    if (disposed) return;
    box = null;
    repaint();
  };
  const Observer = globalThis.ResizeObserver;
  const observer = canvas && typeof Observer === "function" ? new Observer(changed) : null;
  observer?.observe(canvas);

  return {
    read() {
      // Older engines without an observer retain their per-paint size check.
      // An observed zero box wakes through the observer when it becomes visible.
      if (!observer || !box) box = cssBoxOf(canvas);
      return box;
    },
    dispose() {
      disposed = true;
      observer?.disconnect();
      box = null;
    },
  };
}
