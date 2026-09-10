// The git status above the composer, read as characters rather than as text.
//
// The line says four things — ahead, behind, additions, deletions — and each is
// a glyph and a number. Painting it as one string means every change repaints
// the lot, and a line that repaints cannot animate: a digit going 1 → 7 has to
// roll in place, and a section arriving has to cascade in beside the ones that
// were already there.
//
// So the status is a list of cells, one per character, each with a key that
// survives the change. A digit's key counts from the RIGHT of its number, so
// the units column stays the units column when a number grows: 9 → 10 rolls the
// units 9 to 0 and cascades one new cell in, rather than rolling every column.
//
// core/gitStatusTicker.js is what moves them; nothing here touches the DOM.

const SECTIONS = [
  { key: "ahead", glyph: "↑" },
  { key: "behind", glyph: "↓" },
  { key: "insertions", glyph: "+" },
  { key: "deletions", glyph: "−" },
];

const countFrom = (value) => Math.max(0, Math.floor(Number(value) || 0));

/// The counts an older bridge sent as a ready-made diffstat ("+4 −1"). It knows
/// nothing of ahead and behind — those rows carried them nowhere.
function countsFromText(text) {
  const insertions = /\+(\d+)/.exec(text);
  const deletions = /[−-](\d+)/.exec(text);
  return {
    insertions: insertions ? countFrom(insertions[1]) : 0,
    deletions: deletions ? countFrom(deletions[1]) : 0,
  };
}

/** The four numbers the status line is drawn from, whichever shape the feed row
 *  carries them in. */
export function gitCounts(stat) {
  if (!stat) return {};
  if (typeof stat === "string") return countsFromText(stat);
  return {
    ahead: countFrom(stat.ahead),
    behind: countFrom(stat.behind),
    insertions: countFrom(stat.insertions),
    deletions: countFrom(stat.deletions),
  };
}

function sectionCells(section, glyph, count) {
  if (!count) return [];
  const digits = String(count).split("");
  const place = (index) => `d${digits.length - 1 - index}`;
  return [
    { key: `${section}:glyph`, char: glyph, section, slot: "glyph" },
    ...digits.map((char, index) => ({ key: `${section}:${place(index)}`, char, section, slot: "digit" })),
  ];
}

/** Every character the status shows, in the order it reads. */
export function gitStatusCells(stat) {
  const counts = gitCounts(stat);
  return SECTIONS.flatMap(({ key, glyph }) => sectionCells(key, glyph, counts[key]));
}

/** How the line gets from what it says now to what it should say: which cells
 *  leave, which arrive, and which stay put with a new character on them.
 *
 *  The two cascades run in opposite directions, so the order here is the order
 *  they move in: arrivals right to left, departures left to right. */
export function gitStatusPlan(before, after) {
  const was = new Map(before.map((cell) => [cell.key, cell.char]));
  const now = new Map(after.map((cell) => [cell.key, cell.char]));
  return {
    cells: after,
    enters: after.filter((cell) => !was.has(cell.key)).map((cell) => cell.key).reverse(),
    exits: before.filter((cell) => !now.has(cell.key)).map((cell) => cell.key),
    rolls: after
      .filter((cell) => was.has(cell.key) && was.get(cell.key) !== cell.char)
      .map((cell) => ({ key: cell.key, from: was.get(cell.key), to: cell.char })),
  };
}
