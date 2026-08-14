// The floating popovers the review surfaces write into: a "Comment" popover
// anchored to a text selection (plan + diff review), and a note popover
// anchored to a control the reviewer just pressed (a triage disagreement).
//
// Both are the same thing — a small composer that floats over the page, takes
// one short piece of text, and is dismissed by tapping away — so both are one
// popover at a time, positioned the same way, and discarded under the same
// rule: a draft with text in it survives the first tap outside and warns.

import { esc } from "./core/text.js";

let activePop = null;

export function hasCommentPop() {
  return activePop !== null;
}

export function hideCommentPop() {
  if (activePop) {
    if (activePop._onDown) document.removeEventListener("pointerdown", activePop._onDown);
    activePop.remove();
    activePop = null;
  }
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
  return `<textarea class="cp-input" rows="2" placeholder="${esc(placeholder)}"></textarea><button class="cp-save">${esc(confirmLabel)}</button>`;
}

/** The empty popover, placed under `rect` and clamped inside the viewport:
 *  touch selections often end at the screen edge. The composer is taller
 *  (textarea + button + arm hint) and wider (min 240px textarea) than a bare
 *  button, so the clamps leave room for it. */
function openPop(rect) {
  hideCommentPop();
  const pop = document.createElement("div");
  pop.className = "comment-pop";
  pop.style.top = Math.min(rect.bottom + 6, window.innerHeight - 140) + "px";
  pop.style.left = Math.max(8, Math.min(rect.left, window.innerWidth - 300)) + "px";
  document.body.appendChild(pop);

  const onDown = (e) => {
    if (!activePop || activePop.contains(e.target)) return;
    const composer = activePop._composer;
    if (!composer) {
      hideCommentPop();
      return;
    }
    const hasText = composer.input.value.trim() !== "";
    if (outsideTapAction(hasText, composer.isArmed()) === "discard") hideCommentPop();
    else composer.arm();
  };
  setTimeout(() => document.addEventListener("pointerdown", onDown), 0);
  pop._onDown = onDown;
  activePop = pop;
  return pop;
}

/** Turn an open popover into the composer stage. `requireText` is what a
 *  comment needs (an empty comment is nothing) and what an optional note does
 *  not: pressing the button with nothing typed still submits the decision the
 *  note was going to explain. */
function mountComposer(pop, { placeholder, confirmLabel, onSubmit, requireText }) {
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
    const value = input.value.trim();
    if (value || !requireText) onSubmit(value);
    hideCommentPop();
  };
  pop.querySelector(".cp-save").onclick = save;

  input.onkeydown = (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      save();
    } else if (e.key === "Escape") {
      // Escape discards even with text — the deliberate keyboard exit.
      hideCommentPop();
    } else {
      // Any other keystroke (typing/editing) disarms a pending discard.
      disarm();
    }
  };
  // A click inside the pop (e.g. re-focusing the textarea) also disarms.
  pop.addEventListener("pointerdown", disarm);

  pop._composer = { input, arm, isArmed: () => armed };
}

/** The selection popover: a Comment button that opens the composer. */
export function showCommentPop(rect, onAdd) {
  const pop = openPop(rect);
  pop.innerHTML = `<button class="cp-add">💬 Comment</button>`;
  pop.querySelector(".cp-add").onclick = () =>
    mountComposer(pop, {
      placeholder: "Comment on this passage…",
      confirmLabel: "Add",
      onSubmit: onAdd,
      requireText: true,
    });
}

/**
 * The note popover: the composer straight away, for a decision the reviewer has
 * already made by pressing something.
 *
 * `confirmLabel` names the decision rather than the note ("Collapse", not
 * "Save") — the button does the thing, and whatever was typed rides along.
 * Submitting with nothing typed is the ordinary case, not a cancel; tapping
 * outside is the cancel.
 */
export function showNotePop(rect, { placeholder = "Why? (optional)", confirmLabel = "Save", onSubmit }) {
  const pop = openPop(rect);
  mountComposer(pop, { placeholder, confirmLabel, onSubmit, requireText: false });
}
