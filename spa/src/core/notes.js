// Batched review feedback → the single notes document sent to the agent.
// (Task intake is goal-form + batched comments; there is no chat.)

/** Plan-review comments ({snippet, comment}) + general feedback → agent notes. */
export function assemblePlanNotes(comments, general) {
  let out = "Please revise the plan to address this review feedback.\n\n";
  comments.forEach((c, i) => {
    const snippet = c.snippet.replace(/\s+/g, " ").trim();
    out += `${i + 1}. On the passage: "${snippet}"\n   Comment: ${c.comment}\n\n`;
  });
  if (general.trim()) out += `General feedback: ${general.trim()}\n`;
  return out;
}

/** Diff-review comments ({file, lnA, lnB, snippet, comment}) → agent notes.
 *  A 0–0 span is a whole-file comment (the header's ✎ control). */
export function assembleDiffNotes(comments, general) {
  let out = "Please make these changes to the code:\n\n";
  comments.forEach((c, i) => {
    const location =
      c.lnA === 0 && c.lnB === 0 ? "whole file" : c.lnA === c.lnB ? `line ${c.lnA}` : `lines ${c.lnA}-${c.lnB}`;
    out += `${i + 1}. ${c.file} (${location}):\n> ${c.snippet.replace(/\n/g, "\n> ")}\n   Comment: ${c.comment}\n\n`;
  });
  if (general.trim()) out += `General feedback: ${general.trim()}\n`;
  return out;
}

/** Structured plan-thread posts. Anchors survive revisions and bridge restarts. */
export function planThreadMessages(comments, general, revisionId, path) {
  const messages = comments.map((comment) => ({
    body: comment.comment.trim(),
    anchor: {
      artifact: "plan",
      revision_id: revisionId || null,
      path,
      heading_path: comment.headingPath || [],
      snippet: comment.snippet.trim().slice(0, 400),
    },
  }));
  if (general.trim()) messages.push({ body: general.trim(), anchor: null });
  return messages;
}

/** Structured diff-thread posts with a stable, side-aware source location. */
export function diffThreadMessages(comments, general, revisionId) {
  const messages = comments.map((comment) => ({
    body: comment.comment.trim(),
    anchor: {
      artifact: "diff",
      revision_id: revisionId || null,
      path: comment.file,
      side: comment.side || "new",
      line_start: comment.lnA,
      line_end: comment.lnB,
      heading_path: [],
      snippet: comment.snippet.trim().slice(0, 400),
    },
  }));
  if (general.trim()) messages.push({ body: general.trim(), anchor: null });
  return messages;
}
