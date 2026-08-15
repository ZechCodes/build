// The review-comment layer for a stage doc — the doc twin of changesComments.
//
// A comment on a plan is something you said in the conversation, so this holds
// the pending set, draws the same tray the diff surface draws, and sends each
// one as an anchored message (artifact "doc": path, heading path, line range,
// snippet). `issue.comment_add` is the verb that mints it — it posts to the
// issue's thread and answers with the message's id.
//
// It owns no markup beyond the tray and no polling: the mounting controller
// repaints when it likes and calls attach() again, and the layer restores the
// highlights, the general draft and the actions each time. `busy()` is what the
// controller feeds its poll freeze — a rebuild mid-comment would drop the
// anchors, the open popover, and the typed text.

import { commentTrayHtml } from "./changesRender.js";
import { commentLayerBusy } from "./changesModel.js";
import { docLineRange } from "./issueModel.js";
import { buildHeadingPath, slugifyHeading } from "./anchors.js";
import { showCommentPop, hideCommentPop, hasCommentPop } from "../commentPop.js";
import { watchSelection, selectionInside } from "../selectWatch.js";
import { notifyError } from "./notify.js";

/** The rendered heading a marker key names, or null. Keys are heading slugs
 *  (anchors.js), so they are matched as an attribute rather than through an id
 *  selector — a slug can start with a digit, which no id selector accepts. */
export function headingForKey(root, key) {
  if (!root || !key || !/^[-\w]+$/.test(key)) return null;
  const doc = root.querySelector("#stagedoc") || root;
  return doc.querySelector(`[id="${key}"]`);
}

/** The enclosing heading chain for a selection anchor inside a rendered doc:
 *  collect the h1/h2/h3 at or before the anchor node, then reduce to the
 *  enclosing chain (anchors.js). */
export function headingPathFor(docEl, anchorNode) {
  const preceding = Array.from(docEl.querySelectorAll("h1, h2, h3"))
    .filter((h) => h.compareDocumentPosition(anchorNode) & Node.DOCUMENT_POSITION_FOLLOWING || h.contains(anchorNode))
    .map((h) => ({ level: +h.tagName.slice(1), text: h.textContent }));
  return buildHeadingPath(preceding);
}

/**
 * createDocCommentLayer({ submit, docText, hint, onChange }) → the layer.
 *
 * `submit({ comments, general })` sends the assembled posts (the controller
 * supplies the RPCs, since only it knows the issue and stage the doc belongs
 * to); `docText()` is the doc's SOURCE, which is what a line range is measured
 * against; `onChange()` lets the controller repaint when the pending set moves.
 */
export function createDocCommentLayer({
  submit,
  docText = () => "",
  hint = "Select a passage to comment on it. Comments go to the agent as messages.",
  onChange = () => {},
}) {
  const comments = [];
  let nextId = 0;
  let generalDraft = "";
  let host = null;
  let selectionWatcher = null;
  let sending = false;

  const q = (selector) => (host ? host.querySelector(selector) : null);

  /** Re-mark the passages the pending comments name, so what has been said is
   *  visible on the doc and not only in the tray. */
  const applyHighlights = () => {
    if (!host) return;
    host.querySelectorAll(".dochl").forEach((element) => element.classList.remove("dochl"));
    for (const comment of comments) {
      const heading = headingForKey(host, comment.key);
      if (heading) heading.classList.add("dochl");
    }
  };

  const addComment = (headingPath, snippet, body) => {
    const text = String(snippet || "").trim().slice(0, 400);
    const range = docLineRange(docText(), text);
    comments.push({
      id: ++nextId,
      headingPath,
      key: headingPath.length ? slugifyHeading(headingPath[headingPath.length - 1]) : "",
      snippet: text,
      comment: body,
      lineStart: range ? range.line_start : 0,
      lineEnd: range ? range.line_end : 0,
    });
    const selection = window.getSelection();
    if (selection) selection.removeAllRanges();
    onChange();
  };

  const removeComment = (id) => {
    const index = comments.findIndex((comment) => comment.id === id);
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
    if (!comments.length && !generalDraft.trim()) return;
    sending = true;
    renderActions();
    try {
      await submit({ comments: [...comments], general: generalDraft.trim() });
      clear();
      onChange();
    } catch (e) {
      notifyError("Sending comments failed", (e && e.message) || "error");
    } finally {
      sending = false;
      renderActions();
    }
  };

  /** The tray's actionbar: a quiet hint while nothing is pending, Clear + Send
   *  once there is. Re-rendered in place so a repaint is never needed to keep
   *  the buttons honest. */
  function renderActions() {
    const actions = q(".csactions");
    const hintHost = q(".cshint");
    if (!actions || !hintHost) return;
    if (!comments.length && !generalDraft.trim()) {
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
    /** Markup for the tray — the controller drops this under the doc. The tray
     *  is the diff surface's, so a pending comment reads the same wherever it
     *  was written: where it is, what it quotes, what was said. */
    trayHtml: () =>
      commentTrayHtml(
        comments.map((comment) => ({
          id: comment.id,
          file: comment.headingPath.length ? comment.headingPath.join(" › ") : "top of doc",
          lnA: comment.lineStart,
          lnB: comment.lineEnd,
          snippet: comment.snippet,
          comment: comment.comment,
        })),
        { generalDraft },
      ),

    count: () => comments.length,

    /** The reviewer is mid-comment: the controller must freeze its poll. A
     *  passage still being dragged out counts — the popover that turns it into a
     *  comment opens only once it settles, and a rebuild before then takes the
     *  doc the range points into. */
    busy: () =>
      commentLayerBusy({
        pending: comments.length,
        popOpen: hasCommentPop(),
        generalText: generalDraft,
        selecting: Boolean(selectionInside(host)),
      }),

    /** Bind to a freshly-rendered viewer: restore highlights and the general
     *  draft, wire the tray's controls, and watch the doc for selections.
     *  `annotatable` false (a doc that is not readable, or a stage past
     *  commenting) mounts the tray but never the selection watcher. */
    attach(element, { annotatable = true } = {}) {
      host = element;
      if (selectionWatcher) selectionWatcher();
      selectionWatcher = null;
      const docEl = host.querySelector("#stagedoc");
      if (annotatable && docEl) {
        selectionWatcher = watchSelection(docEl, (selection) => {
          const headingPath = headingPathFor(docEl, selection.anchorNode);
          const snippet = selection.toString();
          showCommentPop(selection.getRangeAt(0).getBoundingClientRect(), (body) => addComment(headingPath, snippet, body));
        });
      }
      const general = q(".csgeneral");
      if (general) {
        general.value = generalDraft;
        general.oninput = () => {
          generalDraft = general.value;
          renderActions();
        };
      }
      applyHighlights();
      renderActions();
    },

    /** A click inside the viewer. Returns true when the layer owned it, so the
     *  controller's own handler can stop. */
    handleClick(event) {
      const remove = event.target.closest(".pcx");
      if (!remove) return false;
      removeComment(+remove.dataset.id);
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
