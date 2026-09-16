// The review-comment layer for a changeset, shared by every changeset the
// Changes surface renders — uncommitted, one commit, and (when a view plugs one
// in) the aggregate. It holds the pending comments, draws the tray, and turns a
// selection / a ✎ / a line tap into an anchored conversation post.
//
// It owns no markup of its own beyond the tray (core/changesRender's
// commentTrayHtml) and no polling: the mounting controller repaints when it
// likes and calls attach() again, and the layer restores highlights, the
// general draft, and the actions each time. `busy()` is what the controller
// feeds its poll freeze — a rebuild mid-comment would drop anchors, the open
// popover, and typed text.
//
// Posts go out on the existing pending-comment → thread-post path: anchors
// carry artifact/path/side/line_start/line_end (core/notes.js), and the bridge
// answers with message-<n> ids.

import { commentTrayHtml } from "./changesRender.js";
import { ICON_MESSAGE_SQUARE } from "./icons.js";

/** The gutter's comment button. The same icon the file header wears, because it
 *  is the same verb aimed at one line instead of the whole file. */
const GUTTER_COMMENT_HTML = `<button class="dcmt" type="button" title="Comment on this line" aria-label="Comment on this line">${ICON_MESSAGE_SQUARE}</button>`;
import { pathOf } from "./diff.js";
import { commentLayerBusy } from "./changesModel.js";
import { diffThreadMessages } from "./notes.js";
import { showCommentPop, hideCommentPop, hasCommentPop, openCommentComposer } from "../commentPop.js";
import { watchSelection, selectionInside } from "../selectWatch.js";
import { notifyError } from "./notify.js";

/** The <tr> (with a line number) containing a selection/click node. */
function rowOf(node, root) {
  let element = node && node.nodeType === 3 ? node.parentElement : node;
  while (element && element !== root && element.tagName !== "TR") element = element.parentElement;
  return element && element.tagName === "TR" && element.dataset.ln ? element : null;
}

/**
 * createCommentLayer({ submit, revisionId, onChange }) → the layer.
 *
 * `submit(messages)` sends the assembled thread posts (the controller supplies
 * the RPC); `revisionId()` names the diff revision the anchors belong to (null
 * when the surface has no thread to ask); `onChange()` lets the controller
 * repaint when the pending set changes.
 */
export function createCommentLayer({
  submit,
  revisionId = () => null,
  onChange = () => {},
  readNote = () => "",
}) {
  const comments = [];
  let nextId = 0;
  let host = null;
  let selectionWatcher = null;
  let sending = false;

  const q = (sel) => (host ? host.querySelector(sel) : null);

  const applyHighlights = () => {
    if (!host) return;
    host.querySelectorAll("tr.dhl").forEach((row) => row.classList.remove("dhl"));
    for (const comment of comments) {
      const fileEl = [...host.querySelectorAll(".file")].find((element) => pathOf(element.dataset.key) === comment.file);
      if (!fileEl) continue;
      fileEl.querySelectorAll("tr[data-ln]").forEach((row) => {
        const line = +row.dataset.ln;
        if (line >= comment.lnA && line <= comment.lnB) row.classList.add("dhl");
      });
    }
  };

  const addComment = (file, lnA, lnB, snippet, comment, side = "new") => {
    comments.push({
      id: ++nextId,
      file,
      lnA: lnA || 0,
      lnB: lnB || lnA || 0,
      snippet: String(snippet || "").trim().slice(0, 400),
      comment,
      side,
    });
    const selection = window.getSelection();
    if (selection) selection.removeAllRanges();
    onChange();
  };

  /// The comment button the gutter shows while the pointer is on a line.
  ///
  /// ONE button, moved to whichever row is under the pointer. Rendering one per
  /// row would put thousands of buttons in a long diff — and a diff is long
  /// exactly when it has to stay quick — so the affordance follows the pointer
  /// instead of waiting in every row for a pointer that will never arrive.
  ///
  /// It rides in the row's line-number cell, so it moves with the table as the
  /// code scrolls sideways under it. A repaint that reconciles the row takes it
  /// away, which costs nothing: the next hover puts it back.
  const offerGutterComment = (target) => {
    const cell = commentableGutterCell(target);
    if (!cell) {
      hideGutterComment();
      return;
    }
    if (cell.querySelector(".dcmt")) return;
    hideGutterComment();
    cell.insertAdjacentHTML("beforeend", GUTTER_COMMENT_HTML);
  };

  /** The line-number cell of a row that can actually take a comment: a real
   *  line, in a file whose body is open. A hunk header names no line, and a
   *  capped file's press belongs to the fold. */
  const commentableGutterCell = (target) => {
    const row = target.closest ? target.closest("tr[data-ln]") : null;
    if (!row || row.classList.contains("hunk") || !row.dataset.ln) return null;
    const file = row.closest(".file");
    if (!file || file.classList.contains("capped")) return null;
    return row.querySelector("td.ln");
  };

  const hideGutterComment = () => {
    if (host) host.querySelectorAll(".dcmt").forEach((button) => button.remove());
  };

  const removeComment = (id) => {
    const index = comments.findIndex((c) => c.id === id);
    if (index >= 0) comments.splice(index, 1);
    onChange();
  };

  const clear = () => {
    comments.length = 0;
    hideCommentPop();
  };

  const send = async () => {
    if (sending) return;
    const messages = diffThreadMessages(comments, readNote(), revisionId());
    if (!messages.length) return;
    sending = true;
    renderActions();
    try {
      await submit(messages);
      clear();
      onChange();
    } catch (e) {
      notifyError("Sending comments failed", (e && e.message) || "error");
      throw e;
    } finally {
      sending = false;
      renderActions();
    }
  };

  /** The tray's actionbar: Clear + Send, and nothing at all while there is
   *  nothing to send. It is the TRAY's now — the surface's own git verbs live
   *  in the git toolbar above the stack, where a git verb belongs — so this
   *  bar speaks only about the comments in it. Re-rendered in place so a repaint
   *  is never needed to keep the buttons honest. */
  /** The tray's one control: discarding what has been anchored and not sent.
   *  Sending is the box's, under the diff — there is one place to write and one
   *  button to press, and it is not up here among the comments. */
  function renderActions() {
    const cancel = q(".cscancel");
    if (cancel)
      cancel.onclick = () => {
        clear();
        onChange();
      };
  }

  return {
    /** Markup for the tray — the controller drops this under the diff stack. */
    trayHtml: () => commentTrayHtml(comments),

    count: () => comments.length,

    /** Send what is pending, with whatever note the box under the diff holds.
     *  The box is the surface's (core/changesComposer.js), so the note is read
     *  through `readNote` rather than kept here. */
    send,

    /** The reviewer is mid-comment: the controller must freeze its poll. A
     *  selection still being dragged over the diff counts — the popover that
     *  turns it into a comment opens only once it settles, and a rebuild before
     *  then takes the rows the range points into. */
    busy: () =>
      commentLayerBusy({
        pending: comments.length,
        popOpen: hasCommentPop(),
        generalText: readNote(),
        selecting: Boolean(selectionInside(host)),
      }),

    /** Re-wire the tray's control after a repaint. */
    refreshActions: renderActions,

    /** Whether a send is in flight — what the box under the diff disables its
     *  own button against. */
    sending: () => sending,

    /** Bind to a freshly-rendered changeset: restore highlights, wire the
     *  tray's control, offer the gutter's comment button under the pointer, and
     *  watch for text selections. */
    attach(element) {
      host = element;
      host.onmouseover = (event) => offerGutterComment(event.target);
      if (selectionWatcher) selectionWatcher();
      // eslint-disable-next-line complexity -- ratchet: this callback is at 11, cap 10 — reduce it, then drop this line
      selectionWatcher = watchSelection(host, (selection) => {
        const fileEl = rowOf(selection.anchorNode, host)?.closest(".file");
        if (!fileEl || fileEl.classList.contains("capped")) return;
        const table = fileEl.querySelector("table");
        const startRow = rowOf(selection.anchorNode, table);
        const endRow = rowOf(selection.focusNode, table);
        if (!startRow && !endRow) return;
        let from = +(startRow || endRow).dataset.ln;
        let to = +(endRow || startRow).dataset.ln;
        if (from > to) [from, to] = [to, from];
        const text = selection.toString();
        const side = (startRow || endRow).dataset.side || "new";
        showCommentPop(selection.getRangeAt(0).getBoundingClientRect(), (comment) =>
          addComment(pathOf(fileEl.dataset.key), from, to, text, comment, side),
        );
      });
      host.querySelectorAll(".pcx").forEach((remove) => {
        remove.onclick = () => removeComment(+remove.dataset.id);
      });
      applyHighlights();
      renderActions();
    },

    /** A click inside the changeset. Returns true when the layer owned it, so
     *  the controller's own handler can stop. */
    // eslint-disable-next-line complexity -- ratchet: handleClick is at 12, cap 10 — reduce it, then drop this line
    handleClick(event) {
      const target = event.target;
      const remove = target.closest(".pcx");
      if (remove) {
        removeComment(+remove.dataset.id);
        return true;
      }
      const gutter = target.closest(".dcmt");
      if (gutter) {
        const row = gutter.closest("tr[data-ln]");
        const file = gutter.closest(".file");
        if (row && file)
          openCommentComposer(row.getBoundingClientRect(), (comment) =>
            addComment(pathOf(file.dataset.key), +row.dataset.ln, +row.dataset.ln, row.querySelector(".code").textContent, comment, row.dataset.side || "new"),
          );
        return true;
      }
      const commentButton = target.closest(".fcmt");
      if (commentButton) {
        const fileEl = commentButton.closest(".file");
        if (fileEl)
          openCommentComposer(commentButton.getBoundingClientRect(), (comment) =>
            addComment(pathOf(fileEl.dataset.key), 0, 0, "(entire file)", comment),
          );
        return true;
      }
      const selection = window.getSelection();
      if (selection && !selection.isCollapsed && String(selection).trim()) return false; // the range flow owns it
      const fileEl = target.closest(".file");
      const row = target.closest("tr[data-ln]");
      // A capped file's click belongs to the fold affordance (expand first,
      // comment second) — the same rule the review surfaces already follow.
      if (!fileEl || fileEl.classList.contains("capped")) return false;
      if (!row || row.classList.contains("hunk") || !row.dataset.ln) return false;
      const line = +row.dataset.ln;
      const snippet = row.querySelector(".code").textContent;
      showCommentPop(row.getBoundingClientRect(), (comment) =>
        addComment(pathOf(fileEl.dataset.key), line, line, snippet, comment, row.dataset.side || "new"),
      );
      return true;
    },

    clear,

    dispose() {
      if (selectionWatcher) selectionWatcher();
      selectionWatcher = null;
      hideCommentPop();
      if (host) host.onmouseover = null;
      host = null;
    },
  };
}
