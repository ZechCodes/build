// The reference forms an agent can write, and how prose is read for them.
//
// #56. An agent writing a message, a task body or a comment should be able to
// point at a task, a workspace, another agent or a file without knowing a
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
//   `#fff`, `#aabbcc`    hex colours are letters; a task is digits
//   `hi@example.codes`   email has no LEADING `@`
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

/// A task by its number: `#42`, or one comment on it: `#42/c/tc-7`.
///
/// Digits only, so a hex colour is not a task, and a boundary in front so
/// `file.js#42` and `abc#42` stay prose. The renderer never sees a heading
/// here — that is a line-level rule and this runs inline.
const TASK = /(^|[^\w#&])#(\d+)(?:\/c\/([A-Za-z0-9_-]+))?\b/g;

/// A workspace, an agent or a project: `@workspace:<name or id>`,
/// `@agent:<id>`, `@project:<name or id>` (#229).
///
/// The keyword and the colon are what keep this clear of email (no leading
/// `@`) and of npm scopes (a slash, no colon). The value runs to whitespace or
/// a character that ends a sentence, so `@workspace:build.` links the
/// workspace and leaves the full stop.
/// A name may hold a dot but may not END on one: `@workspace:build.` is a
/// workspace and a full stop, which is what this file's own comment above
/// promised and what the character class quietly did not do — it was greedy to
/// the end of the word, so the reference named "build." and nothing answered
/// for it (#63).
const NAME = "[A-Za-z0-9_-]+(?:[.][A-Za-z0-9_-]+)*";
const NAMED = new RegExp(`(^|[^\\w@/])@(workspace|agent|project):(${NAME})`, "g");

/// What each `@` keyword names, as the fields a caller reads.
const NAMED_FIELDS = {
  workspace: (name) => ({ kind: "workspace", name }),
  agent: (id) => ({ kind: "agent", id }),
  project: (name) => ({ kind: "project", name }),
};

/// Something inside a workspace: `[[<workspace>:<path>]]`, with an optional
/// `#L10` or `#L10-L20`.
///
/// `[[…]]` is free by construction: this renderer has no link syntax of its
/// own, and double brackets are not CommonMark either. Both sides of the colon
/// must carry something, so `[[a.js]]` and `[[ws:]]` are not references.
///
/// The left side may say which source directory of a workspace it means —
/// `[[<workspace>/<directory>:<path>]]` (#229). It is read whole here: a
/// workspace's name is the user's own text and may hold a slash, so which slash
/// splits it is the resolver's question (core/referenceTargets.js).
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

/** What one bracketed reference names: a commit when it says so, else a file. */
function bracketed(workspace, rest) {
  const commit = /^commit:([0-9a-fA-F]{4,40})$/.exec(rest);
  if (commit) return { kind: "commit", workspace, sha: commit[1] };
  if (rest.startsWith("commit:")) return null;
  const place = filePlace(rest);
  return place ? { kind: "file", workspace, ...place } : null;
}

/** Every match of one form, as `{ start, end, fields }`.
 *
 *  `boundary` says whether the form's first group is the character in front of
 *  the reference rather than part of it — how many characters to skip before
 *  the reference itself starts. Asked for, never guessed: it was read off
 *  "group 1 exists", and `[[ws:path]]` has a group 1 that is the WORKSPACE, so
 *  every bracketed reference started ten characters late and the anchor ate
 *  the wrong slice of the line (#63). */
function matchesOf(text, pattern, read, { boundary = true } = {}) {
  const found = [];
  pattern.lastIndex = 0;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    const lead = boundary && match[1] !== undefined ? match[1].length : 0;
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
    ...matchesOf(source, TASK, (match) => ({ kind: "task", number: Number(match[2]), ...(match[3] ? { commentId: match[3] } : null) })),
    ...matchesOf(source, NAMED, (match) => NAMED_FIELDS[match[2]](match[3])),
    ...matchesOf(source, BRACKETED, (match) => bracketed(match[1].trim(), match[2].trim()), { boundary: false }),
  ].sort((one, other) => one.start - other.start);
  return found.map((reference) => ({ ...reference, raw: source.slice(reference.start, reference.end) }));
}
