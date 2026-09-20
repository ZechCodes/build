// The reference forms an agent can write, and how prose is read for them.
//
// #56. An agent writing a message, an issue body or a comment should be able to
// point at an issue, a workspace, another agent or a file without knowing a
// route. This file is the SYNTAX — what a reference looks like and what it
// names. Turning one into a link is core/markdownLinks.js, and the routes are
// core/router.js's; nothing here builds a hash.
//
// # Why these shapes
//
// Three prefixes, one per kind of thing: `#` is the tracker's own numbering,
// which is what people already type; `@` names an actor or a place; `[[…]]` is
// something INSIDE a workspace, at a location.
//
// Each had to survive ordinary writing, because every one of the following is
// in this project's own notes and none of them is a link:
//
//   `# A heading`        a heading needs the space — `#42` has none
//   `#fff`, `#aabbcc`    hex colours are letters; an issue is digits
//   `hi@zech.codes`      email has no LEADING `@`
//   `@anthropic-ai/sdk`  an npm scope: a slash and no colon
//   `gitleaks@8.30.1`    a version specifier
//   `b8ce4ee9`           a bare SHA in prose
//
// Two shapes were proposed and rejected for exactly that reason. `@<workspace>/
// <agent>` is an npm scope. `<workspace>@<sha>` is a version specifier — and
// narrowing the right side to hex rescues most cases but not all, and still
// reads as a version to a person. `@agent:<id>` and `[[<workspace>:commit:
// <sha>]]` say what they mean and cost a word.
//
// Nothing here fires on a bare token. A reference always carries its prefix,
// which is why the workspace is named inside the brackets rather than inferred
// from where the message happens to be.

/// An issue by its number: `#42`.
///
/// Digits only, so a hex colour is not an issue, and a boundary in front so
/// `file.js#42` and `abc#42` stay prose. The renderer never sees a heading
/// here — that is a line-level rule and this runs inline.
const ISSUE = /(^|[^\w#&])#(\d+)\b/g;

/// A workspace or an agent: `@workspace:<name or id>`, `@agent:<id>`.
///
/// The keyword and the colon are what keep this clear of email (no leading
/// `@`) and of npm scopes (a slash, no colon). The value runs to whitespace or
/// a character that ends a sentence, so `@workspace:build.` links the
/// workspace and leaves the full stop.
const NAMED = /(^|[^\w@/])@(workspace|agent):([A-Za-z0-9._-]+)/g;

/// Something inside a workspace: `[[<workspace>:<path>]]`, with an optional
/// `#L10` or `#L10-L20`.
///
/// `[[…]]` is free by construction: this renderer has no link syntax of its
/// own, and double brackets are not CommonMark either. Both sides of the colon
/// must carry something, so `[[a.js]]` and `[[ws:]]` are not references.
const BRACKETED = /\[\[([^\]:[]+):([^\]]+)\]\]/g;

/// The line a bracketed reference ends on, if any. A range opens at its start:
/// the Files route carries `line`, singular (core/router.js `tabPlacePairs`),
/// so `#L10-L20` is honest about where it can actually put the reader.
const LINE = /#L(\d+)(?:-L\d+)?$/;

/** A file's path and the line it opens at, out of a bracketed reference's
 *  second half. */
function filePlace(rest) {
  const line = LINE.exec(rest);
  const path = line ? rest.slice(0, line.index) : rest;
  return path ? { path, ...(line ? { line: Number(line[1]) } : null) } : null;
}

/** What one bracketed reference names: a commit when it says so, else a file.
 *
 *  `commit:` is reserved here and parsed, so the shape is settled — but nothing
 *  links one yet: core/router.js has no route for a commit, and a commit
 *  selection in the Changes view is internal DOM state rather than a URL. The
 *  link layer leaves it as text until that route exists. */
function bracketed(workspace, rest) {
  const commit = /^commit:([0-9a-fA-F]{4,40})$/.exec(rest);
  if (commit) return { kind: "commit", workspace, sha: commit[1] };
  const place = filePlace(rest);
  return place ? { kind: "file", workspace, ...place } : null;
}

/** Every match of one form, as `{ index, length, fields }`. `lead` is how many
 *  characters of the match belong to the boundary in front rather than to the
 *  reference itself. */
function matchesOf(text, pattern, read) {
  const found = [];
  pattern.lastIndex = 0;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    const lead = match[1] === undefined ? 0 : match[1].length;
    const fields = read(match);
    if (fields) found.push({ start: match.index + lead, end: match.index + match[0].length, ...fields });
  }
  return found;
}

/**
 * Every reference in `text`, in the order it was written.
 *
 * Positions are into `text` exactly as given, so a caller can splice without
 * re-finding anything. `raw` is the reference as written, which is what a
 * caller shows when it cannot resolve the target — a reference that names
 * nothing reads as the words the agent typed, never as a broken link.
 */
export function referencesIn(text) {
  const source = text || "";
  const found = [
    ...matchesOf(source, ISSUE, (match) => ({ kind: "issue", number: Number(match[2]) })),
    ...matchesOf(source, NAMED, (match) =>
      match[2] === "workspace" ? { kind: "workspace", name: match[3] } : { kind: "agent", id: match[3] }),
    ...matchesOf(source, BRACKETED, (match) => bracketed(match[1].trim(), match[2].trim())),
  ].sort((one, other) => one.start - other.start);
  return found.map((reference) => ({ ...reference, raw: source.slice(reference.start, reference.end) }));
}
