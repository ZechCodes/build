// Reusable terminal pane — a ghostty-web terminal wired to one keyed PTY on the
// shared terminal socket (§6). Mounts into any host element; dispose() tears down
// the client-side view ONLY. It never closes the server PTY — terminals persist
// across tab switches and reloads; an explicit close is term.close (the tab's ×).

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
 */
export async function mountTerminalPane(host, { attach, input, resize, onExit }) {
  const { Terminal, FitAddon } = await loadGhostty();
  host.innerHTML = "";
  const term = new Terminal({ fontSize: 13, theme: { background: "#15161e", foreground: "#a9b1d6" } });
  term.open(host);
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

  term.onData((data) => input(data).catch(() => {}));
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
      try { term.dispose?.(); } catch { /* ignore */ }
      host.innerHTML = "";
    },
  };
}
