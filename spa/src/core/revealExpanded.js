// Opening a row, and seeing what opened.
//
// A shut row is one line; an open one can be twenty, and the browser does
// nothing about it — the rows below simply move down and the new content hangs
// off the bottom of the screen. So the scroll follows what opened, as far as it
// will go, with one limit: the row's own head never leaves the top edge.
//
// That limit is the whole rule. A reader who presses a line and loses it has to
// find their place again, which costs more than the content they gained; a
// reader who keeps the head can see what they opened and what it belongs to.

/// Whether an element scrolls its own content.
const scrolls = (element) => {
  const view = element.ownerDocument && element.ownerDocument.defaultView;
  if (!view) return false;
  const overflow = view.getComputedStyle(element).overflowY;
  return overflow === "auto" || overflow === "scroll";
};

/// The scroller `element` sits in, or nothing when it sits in the page itself.
function scrollerAround(element) {
  for (let node = element.parentElement; node; node = node.parentElement) {
    if (scrolls(node)) return node;
  }
  return null;
}

/// Bring as much of `block` into `scroller` as its head will allow, and answer
/// how far the scroller moved.
///
/// Only ever downward. A block above the viewport is one the reader scrolled
/// past on purpose, and a row that already fits asks for nothing.
function reveal(scroller, block) {
  const view = scroller.getBoundingClientRect();
  const box = block.getBoundingClientRect();
  const hangingBelow = box.bottom - view.bottom;
  const roomAboveTheHead = box.top - view.top;
  const moves = Math.max(0, Math.min(hangingBelow, roomAboveTheHead));
  if (moves) scroller.scrollTop += moves;
  return moves;
}

/// Scroll `row` into view, through every scroller it is nested in.
///
/// An activity run's rows scroll in their own little window, and that window
/// scrolls in the conversation, so revealing a row inside one means moving
/// both: the row in its box, and then the box in the conversation. The box is
/// what the outer scroller is asked for — the fold's `<details>`, head and all,
/// rather than the bare list inside it.
export function revealExpandedRow(row) {
  let block = row;
  for (let scroller = scrollerAround(block); scroller; scroller = scrollerAround(block)) {
    reveal(scroller, block);
    block = scroller.closest("details") || scroller;
  }
}

/// Wire `scroller` so that opening any row inside it reveals what opened.
///
/// Delegated, and in the capture phase, because `toggle` does not bubble and
/// the row a press lands on is redrawn under it. Wired once: the timeline is
/// wired again on every paint, and a listener added each time would scroll the
/// conversation once per paint the reader has ever sat through.
export function wireExpansionReveal(scroller) {
  if (!scroller || scroller.dataset.revealsExpansions) return;
  scroller.dataset.revealsExpansions = "true";
  scroller.addEventListener(
    "toggle",
    (event) => {
      if (event.target.open) revealExpandedRow(event.target);
    },
    true,
  );
}
