import { esc } from "./text.js";
import { fieldTraits } from "./fieldTraits.js";

/** The durable batch owns text; this controller owns only the active editor. */
export function createReviewInlineComposer({ batch, placeComposer, snapshotNumber }) {
  let active = null;
  let serial = 0;
  const close = () => { active?.row.remove(); active = null; };
  function place() {
    if (!active) return;
    placeComposer?.(active.row, active.draft.anchor);
    if (active.focused && document.activeElement === document.body) active.field.focus({ preventScroll: true });
  }
  function open(draft) {
    close();
    const row = document.createElement("tr");
    row.className = "task-review-composer-row";
    row.dataset.reviewComposerRow = "";
    row.innerHTML = `<td colspan="3"><form class="task-review-inline-composer" data-review-inline-composer>
      <label>${draft.replyTo ? "Reply" : "Comment"} · Revision ${Number(snapshotNumber)}${draft.anchor?.commit ? ` · ${esc(draft.anchor.commit.slice(0, 8))}` : ""} · ${esc(draft.anchor?.path || "Review")} ${draft.anchor ? `· ${esc(draft.anchor.side)} line ${Number(draft.anchor.line)}` : ""}
        <textarea rows="3" ${fieldTraits("prose")} aria-label="Review comment" placeholder="Write a comment…"></textarea>
      </label><div class="row"><button class="btn primary" type="submit">Add to review</button><button class="btn" type="button" data-review-composer-close>Close</button></div>
      <p class="sub">Saved as a draft. Submit the batch when you’re ready.</p></form></td>`;
    const form = row.querySelector("form");
    const field = form.querySelector("textarea");
    field.value = draft.body;
    active = { row, draft, field, focused: false };
    field.onfocus = () => { if (active?.row === row) active.focused = true; };
    field.onblur = () => { if (active?.row === row) active.focused = false; };
    field.oninput = () => { void batch.edit(draft.id, { body: field.value }); };
    form.onsubmit = (event) => { event.preventDefault(); close(); };
    form.querySelector('[data-review-composer-close]').onclick = close;
    place();
    field.focus({ preventScroll: true });
    row.scrollIntoView?.({ block: "nearest" });
  }
  return {
    place, close,
    async comment(anchor, replyTo = null, view = "changes") {
      const generation = ++serial;
      await batch.ready;
      await batch.flush();
      const held = batch.read().drafts.find((draft) => !batch.read().submission?.drafts.some((held) => held.id === draft.id) && (draft.replyTo || null) === replyTo && JSON.stringify(draft.anchor) === JSON.stringify(anchor));
      const id = held?.id || await batch.add({ body: "", anchor, replyTo, view });
      if (generation !== serial) return;
      const draft = batch.read().drafts.find((draft) => draft.id === id);
      if (draft) open(draft);
    },
    edit(draft) { serial++; open(draft); },
    remove(id) { if (active?.draft.id === id) close(); },
    dispose() { serial++; close(); },
  };
}
