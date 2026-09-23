// Move one scroll surface without asking the browser to scroll its ancestors.
export function scrollWithin(scroller, target, { block = "start", behavior = "smooth" } = {}) {
  const surface = scroller.getBoundingClientRect();
  const row = target.getBoundingClientRect();
  const offset = row.top - surface.top - scroller.clientTop;
  const top = scroller.scrollTop + offset;
  if (block === "nearest") {
    if (offset >= 0 && offset + row.height <= scroller.clientHeight) return;
    scroller.scrollTo({ top: offset < 0 ? top : top + row.height - scroller.clientHeight, behavior });
    return;
  }
  const center = block === "center" ? (scroller.clientHeight - row.height) / 2 : 0;
  scroller.scrollTo({ top: top - center, behavior });
}
