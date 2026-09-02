import { esc } from "./text.js";

const MARK_GLYPHS = {
  ok: { glyph: "✓" },
  error: { glyph: "✕", tone: "blocked" },
  unanswered: { glyph: "⊘" },
  pending: { glyph: "○" },
  running: { glyph: "◐" },
  blocked: { glyph: "!", tone: "blocked" },
};

export function outcomeMarkHtml(markName, label) {
  const mark = MARK_GLYPHS[markName];
  if (!mark) return "";
  return `<span class="thread-activity-outcome ${mark.tone || ""}" data-outcome="${esc(markName)}"
    role="img" aria-label="${esc(label)}">${mark.glyph}</span>`;
}
