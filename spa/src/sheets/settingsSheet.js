import { esc } from "../core/text.js";
import "../styles/settingsSheets.css";

/** A settings sheet keeps its identity visible while its controls scroll. */
export function settingsSheetHtml({ title, subtitleHtml = "", bodyHtml = "" }) {
  return `<div class="settings-sheet-frame">
    <header class="settings-sheet-header"><h3>${esc(title)}</h3></header>
    <div class="settings-sheet-body">${subtitleHtml ? `<div class="sub">${subtitleHtml}</div>` : ""}${bodyHtml}</div>
  </div>`;
}
