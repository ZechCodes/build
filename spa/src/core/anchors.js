// Heading-anchor helpers shared by markdown.js (id generation) and the stages
// view (turning a text selection into a { heading_path, snippet } comment anchor).
// Pure — no DOM. The view-side collection of preceding headings lives in the
// stages view; these functions only transform already-collected data.

/**
 * Slugify a raw markdown heading's text into a stable html id fragment: strip
 * inline `backtick` and **bold** markers, lowercase, collapse every run of
 * non-[a-z0-9] to a single "-", and trim leading/trailing dashes. Returns "" for
 * text with no slug-able characters (the caller then omits the id attribute).
 */
export function slugifyHeading(text) {
  return String(text ?? "")
    .replace(/[`*]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/**
 * Compute the enclosing heading chain (outermost first) for an anchor point,
 * given every heading at or before it in document order. Walk backwards taking
 * the nearest heading, then each earlier heading with a strictly smaller level,
 * until an h1 (or the start). `precedingHeadings`: [{ level: 1|2|3, text }].
 */
export function buildHeadingPath(precedingHeadings) {
  const chain = [];
  let level = Infinity;
  for (let i = precedingHeadings.length - 1; i >= 0; i--) {
    const heading = precedingHeadings[i];
    if (heading.level < level) {
      chain.push(heading.text);
      level = heading.level;
      if (level === 1) break;
    }
  }
  return chain.reverse();
}

/**
 * The line range an anchor names, as the `:12` / `:12-18` suffix every review
 * surface writes after a location. An anchor with no range (a whole-file diff
 * comment, a passage whose markers were rendered away) gets "" — the surfaces
 * never print a line 0.
 */
export function lineRangeSuffix(lineStart, lineEnd) {
  const start = Number(lineStart) || 0;
  if (!start) return "";
  const end = Number(lineEnd) || start;
  return end > start ? `:${start}-${end}` : `:${start}`;
}

/**
 * Where an anchored comment points, in one line of text: the enclosing heading
 * chain when it has one, else the artifact's path (the bridge ships it on every
 * comment), else the top of the document — plus the line range. An unanchored
 * message is general: it is about the whole artifact, and says so.
 */
export function anchorLocationLabel(anchor) {
  if (!anchor) return "(general)";
  const headingPath = anchor.heading_path || [];
  const where = headingPath.length ? headingPath.join(" > ") : anchor.path || "(top of doc)";
  return `${where}${lineRangeSuffix(anchor.line_start, anchor.line_end)}`;
}
