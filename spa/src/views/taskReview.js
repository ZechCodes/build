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
import { stampReview, changedSinceReview } from "../core/reviewMemory.js";
import { toggleSecretSpoiler } from "../core/secrets.js";
import { mountSplitButton, createSingleFlight } from "../core/splitButton.js";
import { diffThreadMessages } from "../core/notes.js";
import { currentRevisionId, threadHtml, wireThreadComposer, wireThreadRevisionLinks } from "../core/thread.js";
import { mergeFailureReason, gitActionConfirm } from "../core/taskActions.js";
import { RUN_TERMINAL_STATES } from "../core/board.js";
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
 * createTaskReview({ taskId, callRpc, getTask, absorbTaskView, isOffline, onMerged }) →
 *   { mount(host), unmount() } — the gitPane review plug for a task.
 *
 * getTask() returns the task view's freshest task.get payload (its own poll
 * keeps it current on every tab); absorbTaskView(view) folds an RPC-returned
 * run view back into that cached payload — through the task view's thread
 * cursor cache, so the fold and the next cursored poll agree — and returns
 * the merged task; onMerged() navigates away after a successful merge.
 */
export function createTaskReview({ taskId, callRpc, getTask, absorbTaskView, isOffline, onMerged }) {
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
  // Set by a successful composer send: the next paint must rebuild even
  // through the busy freeze. paint() treats a focused composer as busy, and
  // Cmd+Enter leaves focus in the textarea — without this flag the echo of a
  // just-sent message would never paint (plan.js gets the same guarantee by
  // force-nulling its render key; here busy would still win, so nulling
  // diffKey alone is not enough).
  let forceRebuild = false;
  // The conversation composer's draft, held in the plug closure (like plan.js's
  // threadDraft) and restored into every rebuild — a poll repaint can never eat
  // a half-typed message.
  let threadDraft = "";

  // Re-review memory (W6), per plug instance (per-session): a stamp of what the
  // reviewer saw at their last Request Changes, the files they have ticked off as
  // viewed, and whether the diff is filtered to only what moved since the review.
  // renderedFiles holds the freshest parsed diff so the stamp is taken from it.
  let reviewStamps = new Map();
  const viewedFiles = new Set();
  let changedOnlyFilter = false;
  let renderedFiles = [];

  // ONE single-flight latch for the git split button, owned by the plug — not
  // by each mount. paint() re-runs updateActions() every tick, and without a
  // shared latch each remount would arm a fresh one: mid-merge, the poll would
  // replace the disabled "merging…" button with an enabled Merge that can
  // dispatch a second concurrent run.git_action. The latch is also the freeze
  // key: while active, updateActions leaves the actionbar untouched.
  const gitFlight = createSingleFlight();

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

  const addComment = (file, lnA, lnB, snippet, comment, side = "new") => {
    const idc = ++dcid;
    diffComments.push({ id: idc, file, lnA, lnB, snippet: snippet.trim().slice(0, 400), comment, side });
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
    // A git action (or its confirm modal) is in flight: freeze the actionbar so
    // the poll can never remount an enabled button (or pop a second modal)
    // under the pending RPC.
    if (gitFlight.active()) return;
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
        const messages = diffThreadMessages(
          diffComments,
          q("#dgeneral") ? q("#dgeneral").value : "",
          currentRevisionId(getTask()?.thread, "diff"),
        );
        try {
          await callRpc("run.request_changes", { run_id: taskId, messages });
          // Stamp what we just reviewed: the next pass marks files that moved.
          reviewStamps = stampReview(renderedFiles);
          diffComments.length = 0;
          if (q("#dgeneral")) q("#dgeneral").value = "";
          diffKey = null;
          hideCommentPop();
          paint();
        } catch (e) {
          btn.disabled = false;
          btn.textContent = "Request Changes";
          notifyError("Request Changes failed", e.message);
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
      mountSplitButton(actions, { options: reviewMergeOptions(task && task.adopted, base), run, flight: gitFlight });
    } else if (lastDiffState === "building") {
      hint.textContent = "Comment on the diff to request changes — even while the agent is working.";
      actions.innerHTML = "";
    } else {
      hint.textContent = "";
      actions.innerHTML = "";
    }
  }

  // Wire the conversation composer (thread.post): "ask / tell the agent
  // something" — a durable conversation write that never dispatches a
  // revision pass or moves run state. The request-changes box below it is the
  // other, distinct verb ("send these comments and get a revision"); the copy
  // on each keeps them legible.
  function wireComposer() {
    wireThreadComposer(host, {
      ids: { input: "diffthreadinput", send: "diffthreadsend", hint: "diffthreadhint" },
      readDraft: () => threadDraft,
      writeDraft: (value) => {
        threadDraft = value;
      },
      onSubmit: (body) => callRpc("thread.post", { entity_id: taskId, body }),
      // Optimistic echo: thread.post returns the full updated run view. Fold it
      // back THROUGH the task view's thread cache (never around it, or the next
      // cursored poll would disagree with what we paint), then force the
      // rebuild past the focused-composer freeze.
      afterSubmit: (view) => {
        if (absorbTaskView) absorbTaskView(view);
        diffKey = null;
        forceRebuild = true;
        paint();
      },
      onError: (error) => notifyError("Message failed", error.message),
    });
  }

  function renderBody(t, files) {
    const editable = t.state === "review" || t.state === "building";
    const working = t.state === "building";
    // The diffbar totals always count ALL files; the changed-only filter narrows
    // only what is rendered below. `changed` drives both the per-file chip and the
    // filter membership.
    const totalIns = files.reduce((a, f) => a + f.add, 0),
      totalDel = files.reduce((a, f) => a + f.del, 0);
    const changed = changedSinceReview(reviewStamps, files);
    const filesToRender = changedOnlyFilter ? files.filter((f) => changed.has(f.path)) : files;
    const changedOnlyToggle =
      reviewStamps.size > 0
        ? `<label class="changedonly"><input type="checkbox" id="changedonly"${changedOnlyFilter ? " checked" : ""}/> Only changes since my review</label>`
        : "";
    const filesHtml = filesToRender.length
      ? diffFilesHtml(filesToRender, { commentable: editable, changedSince: changed, viewed: viewedFiles, withViewedToggle: editable })
      : files.length
        ? '<div class="empty">Nothing changed since your review.</div>'
        : '<div class="empty">No file changes yet.</div>';
    host.innerHTML = `
      <div class="diffbar"><span>${files.length} files <span style="color:var(--green)">+${totalIns}</span> <span style="color:var(--red)">−${totalDel}</span></span>
        ${working ? '<span class="dim live-claim">● coding agent working — diff updating live…</span>' : ""}${changedOnlyToggle}</div>
      ${filesHtml}
      ${threadHtml(t.thread, {
        agentLabel: t.harness,
        // The composer shows wherever thread.post can land — the bridge
        // refuses it only once the run is terminal (the conversation is
        // closed), so review and the stage gate get it too. Diff-scoped ids
        // so it can never collide with the plan composer.
        composer: !RUN_TERMINAL_STATES.has(t.state) && {
          inputId: "diffthreadinput",
          sendId: "diffthreadsend",
          hintId: "diffthreadhint",
          placeholder: "Send a message to the coding agent — ask or clarify without requesting a revision…",
        },
      })}
      ${editable ? `<div class="plan-feedback" id="diff-feedback"><div id="difflist"></div>
        <textarea id="dgeneral" class="plan-general" placeholder="Add a general comment about the changes and request updates…"></textarea></div>` : ""}
      <div class="actionbar"><span class="hint" id="diffhint"></span><div class="right" id="diffactions"></div></div>`;
    wireThreadRevisionLinks(host, (revisionId) => callRpc("thread.revision", { entity_id: taskId, revision_id: revisionId }));
    wireComposer();

    // The changed-only filter and the per-file Viewed checkbox live on a delegated
    // change handler: the filter repaints (forcing a rebuild), Viewed collapses the
    // file in place with NO repaint so it survives the poll freeze.
    host.onchange = (e) => {
      const target = e.target;
      if (target.id === "changedonly") {
        changedOnlyFilter = target.checked;
        diffKey = null;
        paint();
        return;
      }
      if (target.classList && target.classList.contains("fviewed-box")) {
        const path = target.dataset.file;
        const fileEl = target.closest(".file");
        if (target.checked) {
          viewedFiles.add(path);
          if (fileEl) {
            fileEl.classList.add("collapsed");
            fileEl.classList.remove("capped");
          }
        } else {
          viewedFiles.delete(path);
          if (fileEl) fileEl.classList.remove("collapsed");
        }
      }
    };

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
        const side = (startRow || endRow).dataset.side || "new";
        showCommentPop(sel.getRangeAt(0).getBoundingClientRect(), (comment) => addComment(file, a, b, text, comment, side));
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
          showCommentPop(tr.getBoundingClientRect(), (comment) => addComment(fileEl.dataset.file, ln, ln, snippet, comment, tr.dataset.side || "new"));
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
    if (!getTask()) return;
    let diff = { stat: { files_changed: 0, insertions: 0, deletions: 0 }, files: [], patch: "" };
    try {
      diff = await callRpc("run.diff", { run_id: taskId });
    } catch {
      return; /* diff not readable yet — the poll retries */
    }
    if (!host) return; // unmounted while the RPC was in flight
    // Re-read AFTER the round trip: a paint that snapshotted the task before
    // awaiting would render pre-post state if the composer posted underneath
    // it, while still consuming the forced-rebuild flag that post set.
    const t = getTask();
    if (!t) return;
    const files = filterNoiseFiles(parseDiff(diff.patch));
    renderedFiles = files; // the freshest parsed diff, for stampReview at Request Changes
    lastDiffState = t.state;
    // Keyed on BOTH sequences: read_unread and the revision-resolution pass
    // stamp seen_at / resolved_by_revision on an already-sequenced message and
    // append nothing, so a key built from creation sequences alone is identical
    // across a mutation and the badge never repaints. updated_sequence is the
    // bridge's marker for exactly that (bridge/src/thread.rs).
    const threadKey =
      t.thread && t.thread.items
        ? t.thread.items.map((item) => `${item.data && item.data.sequence}:${(item.data && item.data.updated_sequence) || 0}`).join(",")
        : "";
    const key = t.state + " " + diff.patch + " " + threadKey;
    const general = q("#dgeneral");
    const composerInput = q("#diffthreadinput");
    // Freeze the diff while the user is actively commenting (pending comments,
    // open popover, or text in the general box) so anchors/selection survive —
    // and skip the rebuild when nothing changed (fold state survives too). A
    // git action in flight freezes too: a rebuild would wipe the busy button.
    // The conversation composer freezes only while focused (typing must not
    // lose the caret); its unfocused draft survives a rebuild via threadDraft,
    // unlike #dgeneral whose content lives only in the DOM.
    const busy =
      gitFlight.active() ||
      diffComments.length > 0 ||
      hasCommentPop() ||
      (general && (general.value.trim() || document.activeElement === general)) ||
      (composerInput && document.activeElement === composerInput);
    if (q(".diffbar") && !forceRebuild && (key === diffKey || busy)) {
      updateActions();
      return;
    }
    forceRebuild = false;
    // A forced rebuild can run while the request-changes box holds unsent
    // text (its content lives only in the DOM): carry it into the fresh box.
    const generalDraft = general ? general.value : "";
    diffKey = key;
    renderBody(t, files);
    const rebuiltGeneral = q("#dgeneral");
    if (rebuiltGeneral && generalDraft && !rebuiltGeneral.value) {
      rebuiltGeneral.value = generalDraft;
      updateActions();
    }
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
      if (host) {
        host.onclick = null;
        host.onchange = null;
      }
      host = null;
    },
  };
}
