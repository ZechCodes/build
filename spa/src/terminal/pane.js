// Reusable terminal pane — a ghostty-web terminal wired to one keyed PTY on the
// shared terminal socket (§6). Mounts into any host element; dispose() tears down
// the client-side view ONLY. It never closes the server PTY — terminals persist
// across tab switches and reloads; an explicit close is term.close (the tab's ×).

import { createTouchScroll, createWheelQuantizer } from "./touchScroll.js";
import { createWheelReporter } from "./mouseWheel.js";

let ghosttyReady = null; // module-level: boot ghostty-web (wasm inlined) once per page.
function loadGhostty() {
  if (!ghosttyReady) ghosttyReady = import("ghostty-web").then(async (m) => { await m.init(); return m; });
  return ghosttyReady;
}

/**
 * mountTerminalPane(host, { attach, input, resize, onExit }) → { dispose, fit, terminal }
 *   attach — (opts) => attach promise: the manager's attachTerminal/attachAgent,
 *            bound with the term_id/task_id (the caller may fold in its own onLive).
 *            The pane supplies onSnapshot/onOutput/onClosed via `opts`.
 *   input  — (data) => promise (term.input)
 *   resize — (cols, rows) => promise (term.resize)
 *   onExit — (reason) => void: tab-level reaction (close the tab / show a quiet chip).
 *   onInputError — (err) => void (optional): a rejected input RPC. Defaults to a
 *            no-op (user terminals swallow it). The agent pane passes a handler
 *            so a keystroke into a dead session surfaces "no active agent
 *            session" instead of silently doing nothing.
 */
export async function mountTerminalPane(host, { attach, input, resize, onExit, onInputError }) {
  const { Terminal, FitAddon } = await loadGhostty();
  host.innerHTML = "";
  const term = new Terminal({ fontSize: 13, theme: { background: "#15161e", foreground: "#a9b1d6" } });
  term.open(host);

  // A PTY app that tracks the mouse (Claude Code: DECSET 1000 + SGR 1006) gets
  // real scroll reports; without this, ghostty's alt-screen fallback turns the
  // wheel into arrow keys, which such apps reject ("use PgUp/PgDn to scroll").
  const wheelReporter = createWheelReporter({
    hasMouseTracking: () => term.hasMouseTracking?.() ?? false,
    isSgr: () => term.getMode?.(1006) ?? false,
    getCellSize: () => {
      const metrics = term.renderer?.getMetrics?.();
      return { width: metrics?.width ?? 9, height: metrics?.height ?? 20 };
    },
    send: (data) => input(data).catch(() => {}), // a dead session drops scrolls quietly
  });
  term.attachCustomWheelEventHandler?.((event) => wheelReporter(event, host.getBoundingClientRect()));

  // ghostty-web registers only mouse/wheel listeners on the host, so on touch
  // devices a drag does nothing. Translate one-finger drags into synthetic
  // pixel-mode wheel events at the host — ghostty's own handleWheel then scrolls
  // scrollback on the normal screen and emits arrow keys on the alternate screen.
  // touch-action:none set here (not CSS) so it travels with every mount site.
  host.style.touchAction = "none";
  const touchScroll = createTouchScroll({
    dispatchWheel: createWheelQuantizer({
      isAltScreen: () => term.buffer.active.type === "alternate",
      // Same fallback as ghostty's own pixel→line conversion when metrics are absent.
      getCellHeight: () => term.renderer?.getMetrics?.()?.height ?? 20,
      emit: (deltaY) => host.dispatchEvent(new WheelEvent("wheel", {
        deltaY, deltaMode: WheelEvent.DOM_DELTA_PIXEL, bubbles: true, cancelable: true,
      })),
    }),
    requestFrame: (cb) => window.requestAnimationFrame(cb),
    cancelFrame: (id) => window.cancelAnimationFrame(id),
  });
  host.addEventListener("touchstart", touchScroll.onTouchStart, { passive: true });
  host.addEventListener("touchmove", touchScroll.onTouchMove, { passive: false });
  // Capture: after a scroll drag, onTouchEnd stops propagation before ghostty's
  // canvas touchend handler focuses the textarea (which would pop the keyboard).
  host.addEventListener("touchend", touchScroll.onTouchEnd, { capture: true });
  host.addEventListener("touchcancel", touchScroll.onTouchCancel);

  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  fitAddon.fit();

  // Refit the terminal (and the PTY) to the host's current box. We compute the
  // target grid with proposeDimensions() but apply it with a direct term.resize()
  // rather than the addon's fit(): fit() carries a _lastCols cache and a 50ms
  // _isResizing guard that, when several triggers fire close together, can swallow
  // the real update. A direct resize is deterministic and idempotent (skipped when
  // the grid is unchanged), and still emits the terminal's onResize → the PTY.
  const fit = () => {
    const dims = fitAddon.proposeDimensions();
    if (!dims || !(dims.cols > 0) || !(dims.rows > 0)) return;
    if (dims.cols !== term.cols || dims.rows !== term.rows) term.resize(dims.cols, dims.rows);
  };

  // A rejected input RPC on a live session is unexpected; on a DEAD session it
  // means the keystroke hit an ended agent — surface it (onInputError) rather
  // than swallow, so the caller can show the idle state. No handler → swallow
  // (a user terminal's transient failures self-heal on the next re-attach).
  term.onData((data) => input(data).catch((err) => onInputError && onInputError(err)));
  // A resize RPC that the bridge rejects on a healthy connection leaves the PTY
  // grid diverged from the rendering, so don't swallow it silently — log it
  // (disconnect/timeout failures still self-heal on the next reconnect's re-attach).
  term.onResize(({ cols, rows }) => resize(cols, rows).catch((err) => console.warn("terminal resize failed:", err)));

  // Expose the most-recently-mounted/focused pane for the QA harness.
  const claim = () => { window.__buildTerminal = term; };
  claim();
  host.addEventListener("focusin", claim);

  await attach({
    cols: term.cols,
    rows: term.rows,
    onSnapshot: (bytes) => { term.reset(); term.write(bytes); },
    onOutput: (bytes) => term.write(bytes),
    onClosed: (reason) => onExit && onExit(reason),
  });

  const resizeObserver = new ResizeObserver(() => fit());
  resizeObserver.observe(host);
  window.addEventListener("resize", fit);

  return {
    terminal: term,
    fit,
    dispose() {
      resizeObserver.disconnect();
      window.removeEventListener("resize", fit);
      host.removeEventListener("focusin", claim);
      host.removeEventListener("touchstart", touchScroll.onTouchStart);
      host.removeEventListener("touchmove", touchScroll.onTouchMove);
      host.removeEventListener("touchend", touchScroll.onTouchEnd, { capture: true });
      host.removeEventListener("touchcancel", touchScroll.onTouchCancel);
      touchScroll.dispose();
      try { term.dispose?.(); } catch { /* ignore */ }
      host.innerHTML = "";
    },
  };
}
