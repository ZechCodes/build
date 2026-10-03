// Checklist edits use the same parser that painted the inputs. Matching raw
// source with a separate regex could tick a fence or an item past the nesting
// bound, and rebuilding a document would change the user's formatting.
import { markdownHtml } from "./markdown.js";

/** Change only the checked character of the rendered checklist at `index`.
 * Invalid indexes and states answer null. Existing checked spelling is kept
 * when the requested state already matches, including an uppercase `X`. */
export function setMarkdownTaskChecked(markdown, index, checked) {
  if (!Number.isInteger(index) || index < 0 || typeof checked !== "boolean") return null;
  const source = String(markdown || "");
  const markers = [];
  markdownHtml(source, { taskMarkers: markers });
  const offset = markers[index];
  if (offset === undefined) return null;
  if ((source[offset] !== " ") === checked) return source;
  return source.slice(0, offset) + (checked ? "x" : " ") + source.slice(offset + 1);
}
