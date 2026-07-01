// A floating "Comment" popover anchored to a text selection (plan + diff review).

let activePop = null;

export function hasCommentPop() {
  return activePop !== null;
}

export function hideCommentPop() {
  if (activePop) {
    if (activePop._onDown) document.removeEventListener("mousedown", activePop._onDown);
    activePop.remove();
    activePop = null;
  }
}

export function showCommentPop(rect, onAdd) {
  hideCommentPop();
  const pop = document.createElement("div");
  pop.className = "comment-pop";
  pop.style.top = rect.bottom + 6 + "px";
  pop.style.left = Math.max(8, rect.left) + "px";
  pop.innerHTML = `<button class="cp-add">💬 Comment</button>`;
  document.body.appendChild(pop);
  pop.querySelector(".cp-add").onclick = () => {
    pop.innerHTML = `<input class="cp-input" placeholder="Comment on this passage…" /><button class="cp-save">Add</button>`;
    const input = pop.querySelector(".cp-input");
    input.focus();
    const save = () => {
      const value = input.value.trim();
      if (value) onAdd(value);
      hideCommentPop();
    };
    pop.querySelector(".cp-save").onclick = save;
    input.onkeydown = (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        save();
      } else if (e.key === "Escape") hideCommentPop();
    };
  };
  const onDown = (e) => {
    if (activePop && !activePop.contains(e.target)) hideCommentPop();
  };
  setTimeout(() => document.addEventListener("mousedown", onDown), 0);
  pop._onDown = onDown;
  activePop = pop;
}
