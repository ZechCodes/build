// Reply previews read only the held timeline. Navigation belongs to a press,
// never to rendering or a cache repaint.
import { esc } from "./text.js";
import { markdownHtml } from "./markdown.js";
import { actorName } from "./trackerLineWords.js";
import { taskAvatarHtml } from "./taskAvatar.js";
import { ageText } from "./trackerChips.js";

export function commentReplyIndex(rows) {
  const comments = new Map();
  const replies = new Map();
  rows.filter((row) => row.type === "comment").forEach((row) => {
    comments.set(row.key, row);
    if (!row.replyTo) return;
    if (!replies.has(row.replyTo)) replies.set(row.replyTo, []);
    replies.get(row.replyTo).push(row.key);
  });
  return { comments, replies };
}

export const commentReplyExcerpt = (parent, context = {}) => markdownHtml(parent.body, {
  mode: "plain", limit: 120, identities: context.identities,
  place: { deviceId: context.deviceId, projectId: context.projectId },
});

export const commentReplyLabel = (parent, context = {}) => parent
  ? `Replying to ${actorName(parent.actor, context)} · ${commentReplyExcerpt(parent, context)}`
  : "Replying to a comment";

export function commentReplyHtml(row, context) {
  if (!row.replyTo) return '<div class="tracker-task-comment-reply" hidden></div>';
  const parent = context.replyIndex.comments.get(row.replyTo);
  if (!parent) return '<div class="tracker-task-comment-reply">Reply to a comment</div>';
  const name = actorName(parent.actor, context);
  const when = parent.at ? `<span class="task-when" title="${esc(parent.at)}">${esc(ageText(parent.at))}</span>` : "";
  return `<button class="tracker-task-comment-reply" type="button" data-comment-jump="${esc(parent.key)}" aria-label="${esc(`Go to the comment by ${name}`)}">
    <span class="task-reply-head">${taskAvatarHtml(parent.actor, context)}<strong class="task-reply-author">${esc(name)}</strong>${when}</span>
    <span class="task-reply-excerpt">${esc(commentReplyExcerpt(parent, context))}</span>
  </button>`;
}

export const commentRepliesHtml = (row, context) => {
  const replies = context.replyIndex.replies.get(row.key) || [];
  return replies.length
    ? `<button class="task-comment-replies" type="button" data-comment-jump="${esc(replies[0])}">${replies.length} ${replies.length === 1 ? "reply" : "replies"}</button>`
    : '<div class="task-comment-replies" hidden></div>';
};

/** A page-owned activation handler. Stable buttons are wired after patching;
 *  no paint moves the scroller, and disposal cancels the arrival mark. */
export function createTaskReplyNavigation(host) {
  let highlighted = null;
  let timer = null;
  const clearHighlight = () => {
    clearTimeout(timer);
    if (highlighted) highlighted.classList.remove("task-comment-target");
    highlighted = null;
  };
  const activate = (event) => {
    const id = event.currentTarget.dataset.commentJump;
    const row = [...host.querySelectorAll(".task-comment[data-comment-id]")]
      .find((candidate) => candidate.dataset.commentId === id);
    if (!row) return;
    clearHighlight();
    // A routed-comment mark belongs to the route and outlives this brief one.
    if (!row.classList.contains("task-comment-target")) highlighted = row;
    row.classList.add("task-comment-target");
    const head = host.querySelector(".task-page-head");
    const headroom = head?.getBoundingClientRect().height || 0;
    const top = host.scrollTop + row.getBoundingClientRect().top - host.getBoundingClientRect().top - host.clientTop - headroom - 12;
    host.scrollTo({ top: Math.max(0, top), behavior: "auto" });
    timer = setTimeout(clearHighlight, 1800);
  };
  const buttons = () => host.querySelectorAll(".task-comment-card button[data-comment-jump]");
  return {
    wire() {
      buttons().forEach((button) => { button.onclick = activate; });
      // The keyed painter owns class attributes; keep a still-live arrival
      // mark until its timer finishes, without repeating the navigation.
      if (highlighted && host.contains(highlighted)) highlighted.classList.add("task-comment-target");
    },
    dispose() {
      buttons().forEach((button) => { button.onclick = null; });
      clearHighlight();
    },
  };
}
