// Anchored review comments → the messages that carry them into a conversation.
//
// Every review surface says the same thing on the wire: a message whose anchor
// names the artifact it points at, the path, the passage it quotes, and — when
// the surface can measure one — the line range. The bridge answers with the
// message's own id (`message-<n>`), and that id IS the comment from then on:
// a persisted comment is withdrawn and resolved by it, and no separate comment
// identity exists any more.
//
// Pure — no DOM, no wire. The mounting controller supplies the RPC.

const SNIPPET_MAX = 400;

/** A quoted passage as it goes on the wire: trimmed and capped. */
function trimSnippet(text) {
  return String(text ?? "").trim().slice(0, SNIPPET_MAX);
}

/**
 * The anchor for one diff comment ({file, lnA, lnB, snippet, side}).
 *
 * A whole-file comment (the file header's ✎, which the surfaces write as a 0–0
 * span) carries NO line range: 0 is not a line, and both ends of the bridge's
 * anchor are optional. Everything else names the span it was written on, on the
 * side of the diff it was written on.
 */
export function diffCommentAnchor(comment, revisionId = null) {
  const start = Number(comment.lnA) || 0;
  const end = Number(comment.lnB) || start;
  return {
    artifact: "diff",
    revision_id: revisionId || null,
    path: comment.file,
    side: comment.side || "new",
    ...(start ? { line_start: start, line_end: end } : {}),
    heading_path: [],
    snippet: trimSnippet(comment.snippet),
  };
}

/** Diff-review comments + the general note → the thread posts that carry them.
 *  The general note is the one message with no anchor: it is about the whole
 *  changeset, not a passage of it. */
export function diffThreadMessages(comments, general, revisionId) {
  const messages = comments.map((comment) => ({
    body: comment.comment.trim(),
    anchor: diffCommentAnchor(comment, revisionId),
  }));
  if (general.trim()) messages.push({ body: general.trim(), anchor: null });
  return messages;
}
