// The one box under the diff.
//
// A reviewer reading a change has two things to say into it: a note to the
// agent, and a commit message. Both are a few sentences about the same diff, so
// both are the same field — and which one it becomes is the button rather than
// a mode the reviewer sets first. Commenting is the primary: it is what a
// review is for, and it is the one that is always safe.
//
// The box sits BELOW the stack rather than at the end of it, so scrolling
// through a long diff never takes it off screen. It opens as one line, because
// most of what goes in it is one line, and grows to a ceiling the stylesheet
// sets per device — a phone gives up half its screen to the keyboard, so its
// ceiling is lower than a computer's.

import { esc } from "./text.js";
import { autoGrow } from "./composer.js";
import { mountSplitButton } from "./splitButton.js";

/** The comment verb's name: what it will do with what is in the box, counting
 *  the anchored comments riding along with it. */
const commentLabel = (pendingComments) =>
  pendingComments ? `Send ${pendingComments} comment${pendingComments === 1 ? "" : "s"}` : "Comment";

/**
 * What the box can do with what is typed in it, primary first.
 *
 * Commenting leads wherever it is on offer. A changeset with no agent to talk
 * to (a historical commit, an unadoptable worktree) can still be committed
 * against; one with nothing uncommitted can only be commented on; a changeset
 * that takes neither gets no box at all.
 */
export function changesComposerOptions({
  commentable = false,
  uncommitted = false,
  pendingComments = 0,
  commitExtras = [],
} = {}) {
  const options = [];
  if (commentable)
    options.push({
      id: "comment",
      label: commentLabel(pendingComments),
      menuLabel: "Comment",
      description: "send this to the coding agent",
      busyLabel: "sending…",
    });
  if (uncommitted)
    options.push({
      id: "commit",
      label: "Commit",
      menuLabel: "Commit",
      description: "commit everything in the worktree with this message",
      busyLabel: "committing…",
    },
    ...commitExtras);
  return options;
}

/** What the empty box says it is for — whichever of the two it can actually
 *  do, named rather than implied. */
export function changesComposerPlaceholder({ commentable = false, uncommitted = false } = {}) {
  if (commentable && uncommitted) return "Comment on these changes, or write a commit message…";
  if (commentable) return "Comment on these changes…";
  return "Commit message…";
}

/** The box: one row of text and the verbs beside it. The ceiling it grows to is
 *  the stylesheet's (`.csbox textarea` max-height), so each device caps its
 *  own. */
export function changesComposerHtml(placeholder) {
  return `<div class="csbox">
    <span class="hint githint"></span>
    <div class="csbox-row">
      <textarea class="csinput" rows="1" placeholder="${esc(placeholder)}"></textarea>
      <div class="csbox-actions"></div>
    </div>
  </div>`;
}

/** Whether two offers ask for the same box — the same verbs, named the same
 *  way. Anything else about the changeset can move without the box being
 *  rebuilt under the reviewer's caret. */
const sameOffer = (a, b) =>
  a.commentable === b.commentable && a.uncommitted === b.uncommitted && a.pendingComments === b.pendingComments;

/** How much of what is on offer is the verbs: a changeset offering neither gets
 *  no box, because there would be nothing to do with what was typed in it. */
const offersNothing = (options) => options.length === 0;

/**
 * Mount the box into `host` and keep it there.
 *
 * `offers()` says what the changeset on screen can take right now; `run(id,
 * text)` does the verb the reviewer picked with what they wrote; `readDraft` /
 * `writeDraft` are the surface's draft slot, so a repaint — or a whole remount
 * — never loses a half-written sentence.
 *
 * `refresh()` re-reads all of that. The FIELD survives it: rebuilding the box
 * because a file changed underneath would take the caret, the selection and the
 * software keyboard with it, so only what actually moved is rewritten.
 */
export function mountChangesComposer(host, { offers, run, readDraft, writeDraft }) {
  let mountedOffer = null;
  let fit = null;

  const input = () => host.querySelector(".csinput");

  const draftNow = () => {
    const box = input();
    return box ? box.value : readDraft();
  };

  /** Run the picked verb with what is in the box, and empty the box once it has
   *  been taken — a sent comment is not still pending, and a commit message is
   *  spent on the commit it made. */
  const runWith = async (id) => {
    const text = draftNow();
    await run(id, text);
    writeDraft("");
    const box = input();
    if (box) {
      box.value = "";
      if (fit) fit();
    }
  };

  const build = (offer) => {
    host.innerHTML = changesComposerHtml(changesComposerPlaceholder(offer));
    const box = input();
    box.value = readDraft();
    box.oninput = () => writeDraft(box.value);
    fit = autoGrow(box);
  };

  const refresh = () => {
    const offer = offers();
    const options = changesComposerOptions(offer);
    if (offersNothing(options)) {
      host.innerHTML = "";
      mountedOffer = null;
      fit = null;
      return;
    }
    if (!mountedOffer) build(offer);
    else if (!sameOffer(mountedOffer, offer)) {
      // The verbs moved, the field did not: only the placeholder is rewritten,
      // and the split button below decides for itself whether it must rebuild.
      input().placeholder = changesComposerPlaceholder(offer);
    }
    mountedOffer = offer;
    mountSplitButton(host.querySelector(".csbox-actions"), { options, run: runWith });
  };

  refresh();
  return { refresh };
}
