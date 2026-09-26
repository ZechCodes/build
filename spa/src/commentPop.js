// The floating popover the review surfaces write into: a "Comment" popover
// anchored to a text selection (plan + diff review), or opened straight onto
// its composer from a control the reviewer pressed.
//
// It is a small composer that floats over the page, takes one short piece of
// text, and is dismissed by tapping away — one popover at a time, and a draft
// with text in it survives the first tap outside and warns.

import { esc } from "./core/text.js";
import { fieldTraits } from "./core/fieldTraits.js";

let activePop = null;

export function hasCommentPop(owner) {
  return activePop !== null && (owner === undefined || activePop._owner === owner);
}

export function hideCommentPop(owner) {
  if (activePop && (owner === undefined || activePop._owner === owner)) {
    if (activePop._onDown) document.removeEventListener("pointerdown", activePop._onDown);
    activePop.remove();
    activePop = null;
  }
}

/**
 * Take the open popover off the page for a surface that is being hidden
 * (views/workspaceChanges.js keeps a directory's surface while another shows).
 * A composer's draft goes with it, disarmed, and nothing of it stays over
 * whatever shows instead; a bare Comment button holds nothing and is just
 * closed. Returns what puts the draft back — over any popover opened since —
 * or null when there was none to keep.
 */
export function suspendCommentPop(owner) {
  const pop = activePop;
  if (!pop || (owner !== undefined && pop._owner !== owner)) return null;
  if (!pop?._composer) {
    hideCommentPop();
    return null;
  }
  pop._composer.disarm();
  document.removeEventListener("pointerdown", pop._onDown);
  pop.remove();
  activePop = null;
  return () => {
    hideCommentPop();
    document.body.appendChild(pop);
    document.addEventListener("pointerdown", pop._onDown);
    activePop = pop;
  };
}

/** Pure decision for an outside tap on the composer: with no text a tap always
 *  discards (nothing to lose); with text the FIRST tap only arms (a warning),
 *  and a SECOND tap while armed discards. Any keystroke or inside click disarms
 *  (handled by the caller). */
export function outsideTapAction(hasText, armed) {
  if (!hasText) return "discard";
  return armed ? "discard" : "arm";
}

/** Markup for the composer stage: a two-line textarea plus its confirm button.
 *  The arm hint (cp-keep) is appended/removed imperatively as the draft arms. */
export function commentComposerHtml(placeholder = "Comment on this passage…", confirmLabel = "Add") {
  return `<textarea class="cp-input" rows="2" ${fieldTraits("prose", "send")} placeholder="${esc(placeholder)}"></textarea><button class="cp-save">${esc(confirmLabel)}</button>`;
}

/** The empty popover, placed under `rect` and clamped inside the viewport:
 *  touch selections often end at the screen edge. The composer is taller
 *  (textarea + button + arm hint) and wider (min 240px textarea) than a bare
 *  button, so the clamps leave room for it. */
function openPop(rect, owner) {
  hideCommentPop();
  const pop = document.createElement("div");
  pop.className = "comment-pop";
  pop._owner = owner;
  pop.style.top = Math.min(rect.bottom + 6, window.innerHeight - 140) + "px";
  pop.style.left = Math.max(8, Math.min(rect.left, window.innerWidth - 300)) + "px";
  document.body.appendChild(pop);

  const onDown = (e) => {
    if (activePop !== pop || pop.contains(e.target)) return;
    const composer = activePop._composer;
    if (!composer) {
      hideCommentPop(pop._owner);
      return;
    }
    const hasText = composer.input.value.trim() !== "";
    if (outsideTapAction(hasText, composer.isArmed()) === "discard") hideCommentPop(pop._owner);
    else composer.arm();
  };
  // From the next turn, so the press that opened it is not its first outside
  // tap — and only while it is still the one open: a popover closed or put
  // away in the turn it opened must not leave a listener acting on the next.
  setTimeout(() => {
    if (activePop === pop) document.addEventListener("pointerdown", onDown);
  }, 0);
  pop._onDown = onDown;
  activePop = pop;
  return pop;
}

/** Turn an open popover into the composer stage. An empty comment is nothing,
 *  so pressing the button with nothing typed submits nothing. */
function mountComposer(pop, { placeholder, confirmLabel, onSubmit }) {
  if (activePop !== pop) return;
  pop.innerHTML = commentComposerHtml(placeholder, confirmLabel);
  const input = pop.querySelector(".cp-input");
  input.focus();

  let armed = false;
  const disarm = () => {
    if (!armed) return;
    armed = false;
    pop.classList.remove("cp-armed");
    const hint = pop.querySelector(".cp-keep");
    if (hint) hint.remove();
  };
  const arm = () => {
    if (armed) return;
    armed = true;
    pop.classList.add("cp-armed");
    const hint = document.createElement("span");
    hint.className = "cp-keep";
    hint.textContent = "tap outside again to discard";
    pop.appendChild(hint);
  };

  const save = () => {
    if (activePop !== pop) return;
    const value = input.value.trim();
    if (value) onSubmit(value);
    hideCommentPop(pop._owner);
  };
  pop.querySelector(".cp-save").onclick = save;

  input.onkeydown = (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      save();
    } else if (e.key === "Escape") {
      // Escape discards even with text — the deliberate keyboard exit.
      hideCommentPop(pop._owner);
    } else {
      // Any other keystroke (typing/editing) disarms a pending discard.
      disarm();
    }
  };
  // A click inside the pop (e.g. re-focusing the textarea) also disarms.
  pop.addEventListener("pointerdown", disarm);

  pop._composer = { input, arm, disarm, isArmed: () => armed };
}

const COMMENT_COMPOSER = { placeholder: "Comment on this passage…", confirmLabel: "Add" };

/**
 * The selection popover: a Comment button that opens the composer.
 *
 * The extra press is the point here. A reader who has just finished dragging
 * out a selection has said nothing yet, and a textarea taking focus the instant
 * they let go would collapse the very selection the comment is about. Pressing
 * Comment is them saying they meant it — and by then the selection is safe.
 */
export function showCommentPop(rect, onAdd, owner) {
  const pop = openPop(rect, owner);
  pop.innerHTML = `<button class="cp-add">💬 Comment</button>`;
  pop.querySelector(".cp-add").onclick = () => mountComposer(pop, { ...COMMENT_COMPOSER, onSubmit: onAdd });
}

/**
 * The composer itself, for a reader who PRESSED something — a file's comment
 * button, a line number.
 *
 * They have already said what they want, so a button that opens a field is a
 * second press for nothing.
 */
export function openCommentComposer(rect, onAdd, owner) {
  mountComposer(openPop(rect, owner), { ...COMMENT_COMPOSER, onSubmit: onAdd });
}
