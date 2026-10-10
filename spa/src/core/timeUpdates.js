/** Keep cosmetic time labels current while their pane and document are visible. */
export function watchTimeUpdates(refresh, intervalMs = 30_000) {
  let visible = true;
  let disposed = false;
  let timer = null;

  const active = () => !disposed && visible && !document.hidden;
  const stopTimer = () => {
    if (timer === null) return;
    clearInterval(timer);
    timer = null;
  };
  const tick = () => {
    if (active()) refresh();
  };
  const sync = () => {
    if (!active()) {
      stopTimer();
      return;
    }
    refresh();
    if (timer === null && active()) timer = setInterval(tick, intervalMs);
  };

  document.addEventListener("visibilitychange", sync);
  window.addEventListener("pageshow", sync);
  sync();

  return {
    setVisible(next) {
      if (disposed || visible === next) return;
      visible = next;
      sync();
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      stopTimer();
      document.removeEventListener("visibilitychange", sync);
      window.removeEventListener("pageshow", sync);
    },
  };
}
