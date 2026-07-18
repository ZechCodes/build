// The task's review surface — the old Diff tab, now the Changes rail's pinned
// "All changes" entry. A gitPane review plug: mount(host) renders the
// aggregate task.diff (vs the base branch) into the detail pane and polls it
// every 1.6s with the same freeze-while-commenting discipline; unmount stops
// the poll. Line/range/file comments accumulate here and go to the coding
// agent via task.request_changes; the merge/commit/push split button lives in
// its actionbar. The plug instance (and its pending comments) belongs to the
// task view, so remounts — tab switches, shell rebuilds — keep review state.

import { esc } from "../core/text.js";
import { parseDiff, filterNoiseFiles } from "../core/diff.js";
import { diffFilesHtml } from "../core/diffRender.js";
import { toggleSecretSpoiler } from "../core/secrets.js";
import { mountSplitButton } from "../core/splitButton.js";
import { assembleDiffNotes } from "../core/notes.js";
import { mergeFailureReason, gitActionConfirm } from "../core/taskActions.js";
import { confirmAction } from "../core/confirm.js";
import { notifyError } from "../core/notify.js";
import { showCommentPop, hideCommentPop, hasCommentPop } from "../commentPop.js";
import { watchSelection } from "../selectWatch.js";

export const REVIEW_POLL_MS = 1600;

// Each option id maps to a task.git_action call. cleanup is omitted for
// commit/push (the bridge rejects cleanup on non-merges).
const GIT_ACTION_RPC = {
  merge_prune: { action: "merge", cleanup: "prune" },
  merge_keep: { action: "merge", cleanup: "keep" },
  merge_release: { action: "merge", cleanup: "release" },
  merge_push: { action: "merge_push", cleanup: "prune" },
  commit: { action: "commit" },
  push: { action: "push" },
};

// The merge option set. Adopted tasks add "Merge & release" (un-adopt after
// merge, keeping the user's worktree). Descriptions carry the raw base
// branch — the split button escapes them.
export function reviewMergeOptions(adopted, base) {
  const options = [
    { id: "merge_prune", label: "Merge", menuLabel: "Merge & clean up", description: `commit, merge into ${base}, remove the worktree + branch`, busyLabel: "merging…" },
    { id: "merge_keep", menuLabel: "Merge & keep worktree", description: `merge into ${base}, keep the worktree and branch`, busyLabel: "merging…" },
  ];
  if (adopted)
    options.push({ id: "merge_release", menuLabel: "Merge & release", description: `merge into ${base}, then un-adopt — keep the worktree and branch, drop the task`, busyLabel: "merging…" });
  options.push(
    { id: "merge_push", menuLabel: "Merge & push", description: `merge, then push ${base} to origin`, busyLabel: "merging & pushing…" },
    { id: "commit", menuLabel: "Commit", description: "commit the work, stay on the branch", busyLabel: "committing…" },
    { id: "push", menuLabel: "Push", description: "commit, then push this branch to origin", busyLabel: "pushing…" },
  );
  return options;
}

/**
 * createTaskReview({ taskId, callRpc, getTask, isOffline, onMerged }) →
 *   { mount(host), unmount() } — the gitPane review plug for a task.
 *
 * getTask() returns the task view's freshest task.get payload (its own poll
 * keeps it current on every tab); onMerged() navigates away after a
 * successful merge.
 */
export function createTaskReview({ taskId, callRpc, getTask, isOffline, onMerged }) {
  let host = null;
  let timer = null;
  let selDispose = null;

  // Review-comment state, preserved across polls AND across mounts (the task
  // view holds one plug instance for its whole life).
  const diffComments = []; // { id, file, lnA, lnB, snippet, comment }
  let dcid = 0,
    diffKey = null,
    lastDiffState = null,
    diffMsg = "";

  const q = (sel) => (host ? host.querySelector(sel) : null);

  // The <tr> (with a line number) containing a selection/click node.
  const rowOf = (node, table) => {
    let element = node && node.nodeType === 3 ? node.parentElement : node;
    while (element && element !== table && element.tagName !== "TR") element = element.parentElement;
    return element && element.tagName === "TR" && element.dataset.ln ? element : null;
  };

  const applyHighlights = () => {
    if (!host) return;
    host.querySelectorAll("tr.dhl").forEach((r) => r.classList.remove("dhl"));
    diffComments.forEach((c) => {
      const fileEl = Array.from(host.querySelectorAll(".file")).find((element) => element.dataset.file === c.file);
      if (!fileEl) return;
      fileEl.querySelectorAll("tr[data-ln]").forEach((tr) => {
        const ln = +tr.dataset.ln;
        if (ln >= c.lnA && ln <= c.lnB) tr.classList.add("dhl");
      });
    });
  };

  const addComment = (file, lnA, lnB, snippet, comment) => {
    const idc = ++dcid;
    diffComments.push({ id: idc, file, lnA, lnB, snippet: snippet.trim().slice(0, 400), comment });
    window.getSelection().removeAllRanges();
    applyHighlights();
    refreshFeedback();
  };
  const removeComment = (idc) => {
    const i = diffComments.findIndex((c) => c.id === idc);
    if (i >= 0) diffComments.splice(i, 1);
    applyHighlights();
    refreshFeedback();
  };

  function refreshFeedback() {
    const list = q("#difflist");
    if (list) {
      list.innerHTML = diffComments
        .map((c) => {
          const location = c.lnA === 0 && c.lnB === 0 ? "" : c.lnA === c.lnB ? `:${c.lnA}` : `:${c.lnA}-${c.lnB}`;
          return `<div class="pcomment"><span class="pcx" data-id="${c.id}">×</span>
            <span class="psnip">${esc(c.file)}${esc(location)} · ${esc(c.snippet.replace(/\s+/g, " ").trim().slice(0, 90))}</span>
            <span class="pctext">${esc(c.comment)}</span></div>`;
        })
        .join("");
      list.querySelectorAll(".pcx").forEach((x) => (x.onclick = () => removeComment(+x.dataset.id)));
    }
    updateActions();
  }

  function updateActions() {
    const actions = q("#diffactions"),
      hint = q("#diffhint");
    if (!actions) return;
    const task = getTask();
    const general = q("#dgeneral") ? q("#dgeneral").value.trim() : "";
    if (diffComments.length || general) {
      hint.textContent = "Your comments will be sent to the coding agent to make changes.";
      actions.innerHTML = `<button class="btn" id="clearrc">Clear</button><button class="btn primary" id="requestChanges">Request Changes</button>`;
      q("#clearrc").onclick = () => {
        diffComments.length = 0;
        if (q("#dgeneral")) q("#dgeneral").value = "";
        applyHighlights();
        refreshFeedback();
      };
      q("#requestChanges").onclick = async () => {
        const btn = q("#requestChanges");
        btn.disabled = true;
        btn.textContent = "requesting…";
        const notes = assembleDiffNotes(diffComments, q("#dgeneral") ? q("#dgeneral").value : "");
        try {
          await callRpc("run.request_changes", { run_id: taskId, comments: notes });
          diffComments.length = 0;
          if (q("#dgeneral")) q("#dgeneral").value = "";
          diffKey = null;
          hideCommentPop();
          paint();
        } catch (e) {
          btn.disabled = false;
          btn.textContent = "Request Changes";
          hint.textContent = "error: " + e.message.slice(0, 50);
        }
      };
    } else if (lastDiffState === "review") {
      // A recent git-action result (Committed./Pushed./error) outlives the poll.
      hint.textContent = diffMsg || "Select code or click a line number to comment, or finish the worktree.";
      // GitHub-style split button: primary runs the default (Merge & clean up),
      // the caret opens the full menu. Every action commits first; push is
      // explicit. Adopted tasks add "Merge & release".
      const base = (task && task.base_branch) || "main";
      const flash = (msg) => {
        diffMsg = msg;
        setTimeout(() => {
          diffMsg = "";
          updateActions();
        }, 6000);
      };
      const run = async (optionId) => {
        const { action, cleanup } = GIT_ACTION_RPC[optionId];
        // Merge variants are decisive: confirm with the exact step outline
        // first. A cancel throws BEFORE any RPC — the split button restores
        // the primary, and no error notice appears.
        const confirmPlan = gitActionConfirm(optionId, {
          branch: (task && task.branch) || "the branch",
          base,
        });
        if (confirmPlan && !(await confirmAction(confirmPlan))) throw new Error("cancelled");
        diffMsg = "";
        const params = { run_id: taskId, action };
        if (cleanup) params.cleanup = cleanup;
        try {
          await callRpc("run.git_action", params);
          if (action === "merge" || action === "merge_push") {
            onMerged();
          } else {
            flash(action === "commit" ? "Committed." : "Pushed " + ((task && task.branch) || "branch") + ".");
            diffKey = null;
            paint();
          }
        } catch (e) {
          // Failures persist as an expandable notice (full message in the
          // detail); successes above stay transient. The hint no longer
          // carries error text — the notice owns it.
          const reason = mergeFailureReason(e.message);
          notifyError(reason ? "Merge failed: " + reason.split("\n")[0] : "Action failed", e.message);
          throw e; // let the split button restore the primary button
        }
      };
      mountSplitButton(actions, { options: reviewMergeOptions(task && task.adopted, base), run });
    } else if (lastDiffState === "building") {
      hint.textContent = "Comment on the diff to request changes — even while the agent is working.";
      actions.innerHTML = "";
    } else {
      hint.textContent = "";
      actions.innerHTML = "";
    }
  }

  function renderBody(t, files) {
    const editable = t.state === "review" || t.state === "building";
    const working = t.state === "building";
    const totalIns = files.reduce((a, f) => a + f.add, 0),
      totalDel = files.reduce((a, f) => a + f.del, 0);
    host.innerHTML = `
      <div class="diffbar"><span>${files.length} files <span style="color:var(--green)">+${totalIns}</span> <span style="color:var(--red)">−${totalDel}</span></span>
        ${working ? '<span class="dim">● coding agent working — diff updating live…</span>' : ""}</div>
      ${files.length ? diffFilesHtml(files, { commentable: editable }) : '<div class="empty">No file changes yet.</div>'}
      ${editable ? `<div class="plan-feedback" id="diff-feedback"><div id="difflist"></div>
        <textarea id="dgeneral" class="plan-general" placeholder="Add a general comment about the changes and request updates…"></textarea></div>` : ""}
      <div class="actionbar"><span class="hint" id="diffhint"></span><div class="right" id="diffactions"></div></div>`;

    if (editable) {
      // Range selections (mouse drag or touch handles) → comment on the span.
      if (selDispose) selDispose();
      selDispose = watchSelection(host, (sel) => {
        const fileEl = rowOf(sel.anchorNode, host)?.closest(".file");
        if (!fileEl) return;
        const file = fileEl.dataset.file,
          table = fileEl.querySelector("table");
        const startRow = rowOf(sel.anchorNode, table),
          endRow = rowOf(sel.focusNode, table);
        if (!startRow && !endRow) return;
        let a = +(startRow || endRow).dataset.ln,
          b = +(endRow || startRow).dataset.ln;
        if (a > b) [a, b] = [b, a];
        const text = sel.toString();
        showCommentPop(sel.getRangeAt(0).getBoundingClientRect(), (comment) => addComment(file, a, b, text, comment));
      });
      // Taps: the header ✎ comments the whole file; a tap on a line comments
      // that line (touch-first path). Capped files leave the click to the
      // pane's fold handler (expand) instead of popping a comment.
      host.onclick = (e) => {
        if (toggleSecretSpoiler(e.target)) return; // reveal/hide a masked dotenv value
        const commentButton = e.target.closest(".fcmt");
        if (commentButton) {
          const fileEl = commentButton.closest(".file");
          if (fileEl)
            showCommentPop(commentButton.getBoundingClientRect(), (comment) =>
              addComment(fileEl.dataset.file, 0, 0, "(entire file)", comment),
            );
          return;
        }
        const sel = window.getSelection();
        if (sel && !sel.isCollapsed && sel.toString().trim()) return; // range flow owns it
        const fileEl = e.target.closest(".file");
        const tr = e.target.closest("tr[data-ln]");
        if (!fileEl || fileEl.classList.contains("capped")) return;
        if (!tr || tr.classList.contains("hunk") || !tr.dataset.ln) return;
        const ln = +tr.dataset.ln,
          snippet = tr.querySelector(".code").textContent;
        showCommentPop(tr.getBoundingClientRect(), (comment) => addComment(fileEl.dataset.file, ln, ln, snippet, comment));
      };
      q("#dgeneral").oninput = updateActions;
    } else {
      host.onclick = null;
    }
    applyHighlights();
    refreshFeedback();
  }

  const paint = async () => {
    if (!host || isOffline()) return;
    const t = getTask();
    if (!t) return;
    let diff = { stat: { files_changed: 0, insertions: 0, deletions: 0 }, files: [], patch: "" };
    try {
      diff = await callRpc("run.diff", { run_id: taskId });
    } catch {
      return; /* diff not readable yet — the poll retries */
    }
    if (!host) return; // unmounted while the RPC was in flight
    const files = filterNoiseFiles(parseDiff(diff.patch));
    lastDiffState = t.state;
    const key = t.state + " " + diff.patch;
    const general = q("#dgeneral");
    // Freeze the diff while the user is actively commenting (pending comments,
    // open popover, or text in the general box) so anchors/selection survive —
    // and skip the rebuild when nothing changed (fold state survives too).
    const busy = diffComments.length > 0 || hasCommentPop() || (general && (general.value.trim() || document.activeElement === general));
    if (q(".diffbar") && (key === diffKey || busy)) {
      updateActions();
      return;
    }
    diffKey = key;
    renderBody(t, files);
  };

  return {
    mount(el) {
      host = el;
      diffKey = null; // a fresh host always needs a first paint
      host.innerHTML = '<div class="empty">loading…</div>';
      paint();
      timer = setInterval(paint, REVIEW_POLL_MS);
    },
    unmount() {
      if (timer) clearInterval(timer);
      timer = null;
      if (selDispose) selDispose();
      selDispose = null;
      hideCommentPop();
      if (host) host.onclick = null;
      host = null;
    },
  };
}
