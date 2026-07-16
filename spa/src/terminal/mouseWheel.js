// Wheel → mouse-protocol forwarding for terminal panes. ghostty-web's own
// wheel handler never reports the mouse to the PTY app: on the alternate
// screen it falls back to arrow-key escapes, which apps that DO track the
// mouse (Claude Code enables DECSET 1000 + SGR 1006) reject with "Scroll
// wheel is sending arrow keys". When tracking is on, the pane intercepts the
// wheel (attachCustomWheelEventHandler) and forwards proper scroll reports.
// DOM-free: all effects are injected so the rules unit-test in node.

const X10_COORD_MAX = 223; // the classic encoding tops out at 255-32
const MAX_REPORTS_PER_EVENT = 5; // matches ghostty-web's own arrow-key cap
const DOM_DELTA_LINE = 1;
const DOM_DELTA_PAGE = 2;
const PAGE_LINES = 24;

const BUTTON_WHEEL_UP = 64;
const BUTTON_WHEEL_DOWN = 65;

/** One mouse scroll report. SGR (mode 1006): `ESC [< b ; col ; row M`.
 *  X10 fallback: `ESC [M` + 32-offset button/col/row bytes, coords clamped to
 *  the encoding's 223 maximum (SGR has no such limit). */
export function wheelReportSequence({ up, col, row, sgr }) {
  const button = up ? BUTTON_WHEEL_UP : BUTTON_WHEEL_DOWN;
  if (sgr) return `\x1b[<${button};${col};${row}M`;
  const clamp = (v) => Math.max(1, Math.min(X10_COORD_MAX, v));
  return "\x1b[M" + String.fromCharCode(32 + button, 32 + clamp(col), 32 + clamp(row));
}

/**
 * createWheelReporter({ hasMouseTracking, isSgr, getCellSize, send }) →
 *   (wheelEvent, hostRect) => boolean
 *
 * Returns the handler for ghostty's attachCustomWheelEventHandler: `true`
 * consumes the event (tracking app owns the wheel), `false` restores the
 * default scrollback/arrow behavior. Sub-cell pixel deltas are banked across
 * events (a slow trackpad drift still scrolls); one report is sent per cell
 * height crossed, capped per event.
 */
export function createWheelReporter({ hasMouseTracking, isSgr, getCellSize, send }) {
  let bankedDelta = 0;
  return (event, hostRect) => {
    if (!hasMouseTracking()) {
      bankedDelta = 0;
      return false;
    }
    const { width, height } = getCellSize();
    const col = Math.max(1, Math.floor((event.clientX - hostRect.left) / width) + 1);
    const row = Math.max(1, Math.floor((event.clientY - hostRect.top) / height) + 1);
    const pixels =
      event.deltaMode === DOM_DELTA_LINE
        ? event.deltaY * height
        : event.deltaMode === DOM_DELTA_PAGE
          ? event.deltaY * height * PAGE_LINES
          : event.deltaY;
    bankedDelta += pixels;
    const owed = Math.trunc(bankedDelta / height);
    if (owed === 0) return true; // consumed — never leak arrows for the remainder
    const reports = Math.max(-MAX_REPORTS_PER_EVENT, Math.min(MAX_REPORTS_PER_EVENT, owed));
    bankedDelta -= reports * height;
    const sgr = isSgr();
    for (let i = 0; i < Math.abs(reports); i++) {
      send(wheelReportSequence({ up: reports < 0, col, row, sgr }));
    }
    return true;
  };
}
