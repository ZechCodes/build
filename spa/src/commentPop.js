// A floating "Comment" popover anchored to a text selection (plan + diff review).

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

/** Markup for the composer stage: a two-line textarea plus the Add button. The
 *  arm hint (cp-keep) is appended/removed imperatively as the draft arms. */
export function commentComposerHtml() {
  return `<textarea class="cp-input" rows="2" placeholder="Comment on this passage…"></textarea><button class="cp-save">Add</button>`;
}

export function showCommentPop(rect, onAdd) {
  hideCommentPop();
  const pop = document.createElement("div");
  pop.className = "comment-pop";
  // Clamp inside the viewport: touch selections often end at the screen edge.
  // The composer is taller (textarea + button + arm hint) and wider (min 240px
  // textarea) than the bare Comment button, so the clamps leave room for it.
  pop.style.top = Math.min(rect.bottom + 6, window.innerHeight - 140) + "px";
  pop.style.left = Math.max(8, Math.min(rect.left, window.innerWidth - 300)) + "px";
  pop.innerHTML = `<button class="cp-add">💬 Comment</button>`;
  document.body.appendChild(pop);

  pop.querySelector(".cp-add").onclick = () => {
    pop.innerHTML = commentComposerHtml();
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
      if (value) onAdd(value);
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
  };

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
}
