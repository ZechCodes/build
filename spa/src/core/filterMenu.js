// A filter menu, as a pure model: what is chosen, what a search leaves, and
// what the press above it says.
//
// Zech, #44: "Would be nice if we used custom drop downs so it would be
// possible to select multiple and have fuzzy search for labels and assignees."
// A native `<select multiple>` is not that control — it has no search, it says
// nothing above itself about what is chosen, and on a phone it is whatever the
// platform decided. So the control is ours, and the half of it that is a
// DECISION rather than a drawing lives here, where a test can read it without
// a DOM.
//
// Two modes, one model. A multi menu holds any number of values and an empty
// selection means "not narrowed"; a single menu holds one or none, and the
// row whose value is the empty string IS the none — "Any column" is a choice
// somebody makes, not the absence of one. So `toggleChoice` answers a list
// either way and the caller never asks which mode it is in.
//
// Ranking is core/fuzzy.js, unchanged: the branch picker already answers "this
// query, against these names, best first" and a second ranker would be a
// second answer to one question.
//
// No DOM, no app imports.

import { fuzzyRank } from "./fuzzy.js";

/**
 * What is chosen after this value was pressed.
 *
 * Multi toggles membership and keeps the caller's order, so a menu does not
 * reshuffle under the reader as they tick. Single replaces — and the empty
 * value empties the selection rather than becoming a chosen `""`, because
 * "Anyone" is not somebody.
 */
export function toggleChoice(chosen, value, { multi = false } = {}) {
  const held = [...(chosen || [])];
  if (!multi) return value === "" ? [] : [value];
  const at = held.indexOf(value);
  if (at === -1) return [...held, value];
  held.splice(at, 1);
  return held;
}

/** The one row that means "do not narrow on this". A multi menu has none —
 *  an empty selection already says it — so it is dropped from the offer and
 *  its words become what the press says while nothing is chosen. */
export const emptyOption = (options) => (options || []).find((option) => option.value === "") || null;

const choosableOptions = (options, multi) =>
  multi ? (options || []).filter((option) => option.value !== "") : [...(options || [])];

/**
 * The rows a search leaves, with the group headings that survive it.
 *
 * A heading is written whenever the group changes rather than once per group:
 * ranking scatters a grouped list, and a reader looking at search results is
 * better served by "this match, and it is in that workspace" than by a tidy
 * outline that lies about the order.
 */
export function menuRows(options, query, chosen, { multi = false, invent = null } = {}) {
  const picked = new Set(chosen || []);
  const ranked = fuzzyRank(choosableOptions(options, multi), query, (option) => option.label);
  const rows = [];
  let heading = null;
  ranked.forEach((option) => {
    const group = option.group || "";
    if (group && group !== heading) rows.push({ kind: "group", key: `group:${group}:${rows.length}`, label: group });
    heading = group;
    rows.push({
      kind: "option",
      key: `option:${option.value}`,
      value: option.value,
      label: option.label,
      checked: picked.has(option.value),
    });
  });
  const coined = inventedRow(options, query, invent);
  if (coined) rows.push(coined);
  return rows;
}

/**
 * The row that makes a name that does not exist yet.
 *
 * A filter chooses from what IS; a composer has to be able to say a label
 * nobody has used before, which is most of what labelling a new issue is. So
 * a menu asked to `invent` offers one extra row at the foot whenever the query
 * names something not already on offer — and it is at the FOOT, because
 * inventing is what you do when none of the answers above it was the one.
 */
function inventedRow(options, query, invent) {
  if (!invent) return null;
  const wanted = String(query || "").trim();
  if (!wanted) return null;
  const known = (options || []).some((option) => option.value === wanted || option.label === wanted);
  if (known) return null;
  return { kind: "option", key: `invent:${wanted}`, value: wanted, label: invent(wanted), checked: false, invented: true };
}

/** Where the option rows are, which is what the arrow keys walk. */
const optionIndexes = (rows) =>
  (rows || []).map((row, index) => (row.kind === "option" ? index : -1)).filter((index) => index >= 0);

/**
 * The row the arrow keys land on next.
 *
 * Headings are stepped over — they are not answers — and the ends STOP rather
 * than wrap: a held arrow key that runs off the bottom and reappears at the
 * top has lost the reader their place in a list they were reading.
 */
export function moveActive(rows, active, step) {
  const indexes = optionIndexes(rows);
  if (!indexes.length) return -1;
  const at = indexes.indexOf(active);
  if (at === -1) return step > 0 ? indexes[0] : indexes[indexes.length - 1];
  const next = at + (step > 0 ? 1 : -1);
  if (next < 0 || next >= indexes.length) return indexes[at];
  return indexes[next];
}

/** The first row a fresh search should sit on. */
export const firstActive = (rows) => (optionIndexes(rows)[0] ?? -1);

const labelOf = (options, value) =>
  (options || []).find((option) => option.value === value)?.label || value;

/**
 * What the press says about what is chosen.
 *
 * Nothing chosen is the empty row's own words — "Anyone", "Any label" — so the
 * bar reads as a sentence about the list rather than as four blanks. One thing
 * chosen is that thing, named in full.
 *
 * More than one is said two ways, because two kinds of name want different
 * things. Labels are short and interchangeable, so the filter names ITSELF and
 * counts them: `Labels · 2`. An assignee is a person or an agent and the first
 * one is the most useful word on the bar, so it leads and the rest are a
 * number: `issues-spa · Agent 1 +1`.
 */
export function menuPressLabel({ name, options, chosen, summary = "first" }) {
  const held = chosen || [];
  if (!held.length) return emptyOption(options)?.label || name;
  if (held.length === 1) return labelOf(options, held[0]);
  if (summary === "count") return `${name} · ${held.length}`;
  return `${labelOf(options, held[0])} +${held.length - 1}`;
}

/** What the reader is told a search box searches. Said out loud rather than
 *  only placed, because a placeholder is not an accessible name. */
export const searchLabel = (name) => `Search ${String(name || "").toLowerCase()}`;
