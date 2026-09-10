// What a paint cost, said out loud when it cost too much.
//
// The surfaces here repaint on a poll, so a paint that has quietly grown into
// tens of milliseconds is invisible until the whole app feels heavy. Every
// paint that matters goes through this wrapper: it lands in the performance
// timeline under its own name (so a devtools profile reads as the app's own
// vocabulary rather than anonymous script time) and it warns in the console
// the moment one crosses the frame budget.

/** The budget one paint gets before it is worth complaining about: six frames
 *  at 60Hz, which is the point a repaint stops feeling like a repaint. */
export const SLOW_PAINT_MS = 100;

const MARK_PREFIX = "build:";

const timelineOf = () => {
  const clock = globalThis.performance;
  return clock && typeof clock.mark === "function" && typeof clock.now === "function" ? clock : null;
};

/** Take the marks back out of the buffer: a surface that paints every 1.6s
 *  would otherwise grow the timeline for as long as the tab is open. */
function forget(clock, names) {
  if (typeof clock.clearMarks === "function") names.forEach((name) => clock.clearMarks(name));
  if (typeof clock.clearMeasures === "function") clock.clearMeasures(names[names.length - 1]);
}

/** The three names one paint occupies in the timeline. */
function paintMarks(name) {
  return { start: `${MARK_PREFIX}${name}:start`, end: `${MARK_PREFIX}${name}:end`, measure: `${MARK_PREFIX}${name}` };
}

function reportPaint(clock, name, marks, milliseconds) {
  clock.mark(marks.end);
  clock.measure(marks.measure, marks.start, marks.end);
  if (milliseconds > SLOW_PAINT_MS) console.warn(`slow paint: ${name} took ${Math.round(milliseconds)}ms`);
  forget(clock, [marks.start, marks.end, marks.measure]);
}

/** Run `paint`, answering exactly what it answers. A paint that throws is
 *  measured and then throws on — the timeline says what it cost either way. */
export function timedPaint(name, paint) {
  const clock = timelineOf();
  if (!clock) return paint();
  const marks = paintMarks(name);
  clock.mark(marks.start);
  const startedAt = clock.now();
  try {
    return paint();
  } finally {
    reportPaint(clock, name, marks, clock.now() - startedAt);
  }
}
