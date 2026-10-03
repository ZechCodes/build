// Minimal, safe markdown → HTML: the ONE way the SPA renders markdown (#229).
// Everything is HTML-escaped; only the small vocabulary below is rendered.
//
// Every surface that shows something an agent or a person wrote — a chat
// message, a task body, a comment, a plan doc, a file preview, a one-line
// preview — calls `markdownHtml`, and nothing else in this file is exported.
// So the escaping, the vocabulary and the references (core/markdownRefs.js)
// are the same everywhere, and a reference resolves against the one index
// every surface shares (core/referenceIndex.js). A surface's own shape — a
// line, a block, a preview — is a `mode`, not a renderer of its own.
// spa/test/markdownEntry.test.js fails a module that reaches around it.

import { esc } from "./text.js";
import { blocksHtml } from "./markdownBlocks.js";
import { expandReferences, plainReferences } from "./markdownLinks.js";
import { webLinks } from "./markdownWebLinks.js";
import { plainPreview } from "./previewText.js";
import { referenceResolver } from "./referenceIndex.js";

/** One line's inline vocabulary — code spans, strong, web links
 *  (core/markdownWebLinks.js) and the references (core/markdownLinks.js) —
 *  over escaped text. Links are read AFTER the code spans and never inside
 *  one: a message explaining this syntax is mostly examples, and they have to
 *  stay literal. Web links go before references, so a `#42` inside a link's
 *  label or address stays part of that link. */
const inlineHtml = (text, links) =>
  expandReferences(
    webLinks(
      esc(text)
        .replace(/`([^`]+)`/g, "<code>$1</code>")
        .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>"),
    ),
    links,
  );

/** The shapes a surface can ask for. `block` is a document: paragraphs,
 *  headings, lists, tables, fences. `inline` is one line of the inline
 *  vocabulary with its lines joined, for a row or a card that holds no blocks.
 *  `plain` is text, not HTML: one line with the marks taken off and each
 *  reference read as its words (core/previewText.js), cut at `limit`. */
const MODES = {
  block: (text, links, limit, options) => blocksHtml(text, (line) => inlineHtml(line, links), options),
  inline: (text, links) => inlineHtml(text.trim().replace(/\s*\n\s*/g, " "), links),
  plain: (text, links, limit) => plainPreview(plainReferences(text, links), limit),
};

/**
 * Markdown for a surface to show — the one entry point (#229).
 *
 * `place` is where the reader stands (`{ deviceId, projectId }`): the project a
 * bare `#42` is read in, and the one a name is looked for first. Without one,
 * references that name something account-wide — a workspace, an agent, a
 * project — still resolve. `identities` is what the surface knows about agents
 * beyond the feed (a task carries every actor on it). `mode` is one of
 * `MODES`; `limit` caps a plain preview. List checklist inputs are disabled
 * unless `taskItems` is true. `taskMarkers` collects their original source
 * offsets for the source-edit helper; it follows the same block traversal.
 * `taskLabelScope` is a stable, unique name for this rendered document. Task
 * bodies and comments supply one to label checkboxes from their rendered text
 * without changing ids during a cached repaint. Other callers keep stable,
 * id-free markup with accessible names derived from the rendered inline text.
 *
 * Answers HTML for `block` and `inline`, and plain text for `plain`.
 */
export function markdownHtml(text, { place = null, identities = null, mode = "block", limit, taskItems = false, taskMarkers, taskLabelScope } = {}) {
  const links = referenceResolver({ place, identities: identities || {} });
  return (MODES[mode] || MODES.block)(String(text || ""), links, limit, { taskItems, taskMarkers, taskLabelScope });
}
