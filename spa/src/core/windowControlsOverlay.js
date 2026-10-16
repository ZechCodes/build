// Standard, read-only window geometry. No Electron globals or privileged bridge
// enter the hosted renderer. Browsers and standalone PWAs report no overlay.
export function installWindowControlsOverlay() {
  const overlay = navigator.windowControlsOverlay;
  if (!overlay) return () => {};
  const root = document.documentElement;
  const clear = () => {
    root.removeAttribute("data-window-controls-overlay");
    root.style.removeProperty("--window-controls-x");
  };
  const sync = () => {
    const rect = overlay.getTitlebarAreaRect();
    if (!overlay.visible || rect.width <= 0 || rect.height <= 0) return clear();
    root.setAttribute("data-window-controls-overlay", "");
    // CSS env() is authoritative in Electron. The API value is also useful to
    // hosts exposing geometry before the environment variable is available.
    root.style.setProperty("--window-controls-x", `${rect.x}px`);
  };
  sync();
  overlay.addEventListener("geometrychange", sync);
  return () => {
    overlay.removeEventListener("geometrychange", sync);
    clear();
  };
}
