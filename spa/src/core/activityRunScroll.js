// An open activity run shows what the agent is doing NOW.
//
// A run's rows scroll in their own window, eight rows tall, and the reconciler
// keeps that window across every repaint — so a box opened on its oldest row
// stays on its oldest row while the agent writes twenty more into the bottom of
// it. Opening on the oldest is opening on the least interesting thing in it.
//
// So a run's box follows its newest row, on the same terms the conversation
// above it follows its newest message: it holds the end unless the reader
// scrolled up inside it, and a reader who did is reading.

/** How near the end still counts as watching the end. Absorbs the fractional
 *  scroll heights a zoomed or sub-pixel layout leaves behind. */
const AT_END_SLACK_PX = 8;

const RUN = "[data-activity-run]";
const RUN_LIST = ".thread-activity-group-list";

const runKeyOf = (list) => list.closest(RUN).getAttribute("data-activity-run");

const openRunLists = (scroller) => [...scroller.querySelectorAll(`${RUN} > ${RUN_LIST}`)];

const atEnd = (list) => list.scrollHeight - list.clientHeight - list.scrollTop <= AT_END_SLACK_PX;

/// Run `paint`, leaving every open run showing its newest row.
///
/// A box that was at its end before the paint is put back at its end after it,
/// and a box the paint has just drawn opens there — the newest row is what the
/// reader pressed the run to see. Every other box is left exactly where the
/// reader put it.
export function paintRunsShowingLatest(scroller, paint) {
  if (!scroller) {
    paint();
    return;
  }
  const watching = new Set(openRunLists(scroller).filter(atEnd).map(runKeyOf));
  const known = new Set(openRunLists(scroller).map(runKeyOf));
  paint();
  for (const list of openRunLists(scroller)) {
    const key = runKeyOf(list);
    if (watching.has(key) || !known.has(key)) list.scrollTop = list.scrollHeight;
  }
}
