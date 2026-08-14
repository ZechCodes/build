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
import { commentLayerBusy } from "./changesModel.js";
import { diffThreadMessages } from "./notes.js";
import { showCommentPop, hideCommentPop, hasCommentPop } from "../commentPop.js";
import { watchSelection } from "../selectWatch.js";
import { notifyError } from "./notify.js";

/** The <tr> (with a line number) containing a selection/click node. */
function rowOf(node, root) {
  let element = node && node.nodeType === 3 ? node.parentElement : node;
  while (element && element !== root && element.tagName !== "TR") element = element.parentElement;
  return element && element.tagName === "TR" && element.dataset.ln ? element : null;
}

/**
 * createCommentLayer({ submit, revisionId, hint, onChange }) → the layer.
 *
 * `submit(messages)` sends the assembled thread posts (the controller supplies
 * the RPC); `revisionId()` names the diff revision the anchors belong to (null
 * when the surface has no thread to ask); `hint` is the resting actionbar copy;
 * `onChange()` lets the controller repaint when the pending set changes.
 */
export function createCommentLayer({
  submit,
  revisionId = () => null,
  hint = "Select code, tap a line, or use ✎ to comment. Comments go to the agent.",
  onChange = () => {},
}) {
  const comments = [];
  let nextId = 0;
  let generalDraft = "";
  let host = null;
  let selectionWatcher = null;
  let sending = false;

  const q = (sel) => (host ? host.querySelector(sel) : null);

  const applyHighlights = () => {
    if (!host) return;
    host.querySelectorAll("tr.dhl").forEach((row) => row.classList.remove("dhl"));
    for (const comment of comments) {
      const fileEl = [...host.querySelectorAll(".file")].find((element) => element.dataset.file === comment.file);
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

  const removeComment = (id) => {
    const index = comments.findIndex((c) => c.id === id);
    if (index >= 0) comments.splice(index, 1);
    onChange();
  };

  const clear = () => {
    comments.length = 0;
    generalDraft = "";
    hideCommentPop();
  };

  const send = async () => {
    if (sending) return;
    const messages = diffThreadMessages(comments, generalDraft, revisionId());
    if (!messages.length) return;
    sending = true;
    renderActions();
    try {
      await submit(messages);
      clear();
      onChange();
    } catch (e) {
      notifyError("Sending comments failed", (e && e.message) || "error");
    } finally {
      sending = false;
      renderActions();
    }
  };

  /** The tray's actionbar: quiet hint while nothing is pending, Clear + Send
   *  once there is. Re-rendered in place so a repaint is never needed to keep
   *  the buttons honest. */
  function renderActions() {
    const actions = q(".csactions");
    const hintHost = q(".cshint");
    if (!actions || !hintHost) return;
    const pending = comments.length > 0 || generalDraft.trim().length > 0;
    if (!pending) {
      hintHost.textContent = hint;
      actions.innerHTML = "";
      return;
    }
    const count = comments.length;
    hintHost.textContent = count
      ? `${count} comment${count === 1 ? "" : "s"} ready to send.`
      : "Your note goes to the agent.";
    actions.innerHTML = `<button class="btn cscancel">Clear</button><button class="btn primary cssend"${sending ? " disabled" : ""}>${sending ? "sending…" : "Send to agent"}</button>`;
    const cancel = actions.querySelector(".cscancel");
    if (cancel)
      cancel.onclick = () => {
        clear();
        onChange();
      };
    const sendButton = actions.querySelector(".cssend");
    if (sendButton) sendButton.onclick = send;
  }

  return {
    /** Markup for the tray — the controller drops this under the diff stack. */
    trayHtml: () => commentTrayHtml(comments, { generalDraft }),

    count: () => comments.length,

    /** The reviewer is mid-comment: the controller must freeze its poll. */
    busy: () => commentLayerBusy({ pending: comments.length, popOpen: hasCommentPop(), generalText: generalDraft }),

    /** Bind to a freshly-rendered changeset: restore highlights and the general
     *  draft, wire the tray's controls, and watch for text selections. */
    attach(element) {
      host = element;
      if (selectionWatcher) selectionWatcher();
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
          addComment(fileEl.dataset.file, from, to, text, comment, side),
        );
      });
      const general = q(".csgeneral");
      if (general) {
        general.value = generalDraft;
        general.oninput = () => {
          generalDraft = general.value;
          renderActions();
        };
      }
      host.querySelectorAll(".pcx").forEach((remove) => {
        remove.onclick = () => removeComment(+remove.dataset.id);
      });
      applyHighlights();
      renderActions();
    },

    /** A click inside the changeset. Returns true when the layer owned it, so
     *  the controller's own handler can stop. */
    handleClick(event) {
      const target = event.target;
      const remove = target.closest(".pcx");
      if (remove) {
        removeComment(+remove.dataset.id);
        return true;
      }
      const commentButton = target.closest(".fcmt");
      if (commentButton) {
        const fileEl = commentButton.closest(".file");
        if (fileEl)
          showCommentPop(commentButton.getBoundingClientRect(), (comment) =>
            addComment(fileEl.dataset.file, 0, 0, "(entire file)", comment),
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
        addComment(fileEl.dataset.file, line, line, snippet, comment, row.dataset.side || "new"),
      );
      return true;
    },

    clear,

    dispose() {
      if (selectionWatcher) selectionWatcher();
      selectionWatcher = null;
      hideCommentPop();
      host = null;
    },
  };
}
