// A worktree's review surface — the merge-base-anchored diff of everything the
// worktree carries, mounted as the Changes rail's pinned "All changes" entry
// (the gitPane review plug, the same contract taskReview.js implements). Line,
// range, and file comments accumulate here; sending them — like any Merge or
// Abandon from its actionbar — transparently adopts the worktree as a task
// first. Everything rendered from the worktree's branch, subject, path, and
// diff is UNTRUSTED and escaped.
//
// The plug instance belongs to the worktree view, so remounts (tab switches,
// rail selection moving away and back) keep pending review comments.

import { esc } from "../core/text.js";
import { parseDiff, filterNoiseFiles } from "../core/diff.js";
import { diffFilesHtml } from "../core/diffRender.js";
import { diffThreadMessages } from "../core/notes.js";
import { loadModelCatalog } from "../app.js";
import { mountSplitButton } from "../core/splitButton.js";
import { gitActionConfirm, abandonConfirm, mergeFailureReason } from "../core/taskActions.js";
import { confirmAction } from "../core/confirm.js";
import { notifyError } from "../core/notify.js";
import { showCommentPop, hideCommentPop, hasCommentPop } from "../commentPop.js";
import { watchSelection } from "../selectWatch.js";
import { toggleSecretSpoiler } from "../core/secrets.js";
import { whenVisible } from "../core/visibility.js";
import {
  catalogForProvider,
  effortOptionsHtml,
  modelInCatalog,
  modelOptionsHtml,
  modelParams,
  normalizeModelCatalog,
  providerOptionsHtml,
} from "../core/modelPicker.js";

export const WORKTREE_REVIEW_POLL_MS = 15000;

// The adopted-task merge set for the browse view: prune / keep / release only
// (no commit/push/merge_push here — those belong to a task already in review;
// committing and pushing this worktree is what the Changes surface around this
// plug is for).
const WORKTREE_MERGE_OPTIONS = [
  { id: "merge_prune", label: "Merge", menuLabel: "Merge & clean up", description: "commit, merge into the base branch, remove the worktree + branch", busyLabel: "merging…" },
  { id: "merge_keep", menuLabel: "Merge & keep worktree", description: "merge into the base branch, keep the worktree and branch", busyLabel: "merging…" },
  { id: "merge_release", menuLabel: "Merge & release", description: "merge into the base branch, then un-adopt — keep the worktree and branch, drop the task", busyLabel: "merging…" },
];
const MERGE_RPC = {
  merge_prune: { action: "merge", cleanup: "prune" },
  merge_keep: { action: "merge", cleanup: "keep" },
  merge_release: { action: "merge", cleanup: "release" },
};

/**
 * createWorktreeReview({ projectId, worktreeId, callRpc, adopting, isOffline,
 *                        onAdopted, onFinished, onGone }) →
 *   { mount(host), unmount(), getBase() } — the gitPane review plug.
 *
 * `adopting` is the view's shared createAdoptingCall: the first mutating action
 * binds the worktree to a task, and every caller must agree on which task that
 * is. `onAdopted()` hands off to it, `onFinished()` leaves the surface after a
 * merge or abandon, and `onGone()` fires when the worktree stops resolving
 * without this surface having adopted it.
 */
export function createWorktreeReview({
  projectId,
  worktreeId,
  callRpc,
  adopting,
  initialProvider = "",
  isOffline = () => false,
  onAdopted = () => {},
  onFinished = () => {},
  onGone = () => {},
}) {
  let host = null;
  let timer = null;
  let selDispose = null;
  // A mutating action (request changes / merge / abandon) is running: it is
  // about to adopt or remove this worktree, so the poll must not race it to a
  // "the worktree vanished" verdict.
  let acting = false;

  // Review-comment state, preserved across polls AND across mounts (the view
  // holds one plug instance for its whole life).
  const diffComments = []; // { id, file, lnA, lnB, snippet, comment }
  let dcid = 0;
  let diffKey = null;
  let meta = null; // the last worktree.diff payload's branch/base/adoptable/path

  let agentCatalog = normalizeModelCatalog({});
  const agentChoice = { provider: initialProvider || "claude", model: "", effort: "" };
  loadModelCatalog().then((catalog) => {
    agentCatalog = normalizeModelCatalog(catalog);
    // A provider chosen for THIS worktree — in the sheet, before the directory
    // existed — outranks the account default: it is the agent the human asked
    // to run here, and adoption dispatches with it.
    if (!initialProvider) agentChoice.provider = agentCatalog.default_provider || "claude";
    updateActions();
  });

  const q = (sel) => (host ? host.querySelector(sel) : null);
  const branchLabel = () => (meta && meta.branch) || "the branch";
  const baseLabel = () => (meta && meta.base_branch) || "main";

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
    diffComments.push({ id: ++dcid, file, lnA: lnA || lnB, lnB: lnB || lnA, snippet: snippet.trim().slice(0, 400), comment });
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
    const list = q("#wdifflist");
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

  /** Run a mutating action with the poll frozen: it adopts (or removes) this
   *  worktree, after which worktree.diff resolves to "unknown worktree_id" —
   *  a poll landing mid-action would read that as "the worktree vanished". */
  const act = async (work) => {
    acting = true;
    try {
      return await work();
    } finally {
      acting = false;
    }
  };

  function updateActions() {
    const actions = q("#wdiffactions"),
      hint = q("#wdiffhint");
    if (!actions) return;
    const general = q("#wgeneral") ? q("#wgeneral").value.trim() : "";
    if (diffComments.length || general) {
      hint.textContent = "Your comments adopt this worktree as a task and are sent to the coding agent.";
      const providerCatalog = catalogForProvider(agentCatalog, agentChoice.provider);
      const selectedModel = modelInCatalog(providerCatalog.models, agentChoice.model);
      actions.innerHTML = `<select class="mini" id="wprovider" title="Coding agent">${providerOptionsHtml(agentCatalog.providers, agentChoice.provider)}</select>
        <select class="mini" id="wmodel" title="Coding agent model">${modelOptionsHtml(providerCatalog.models, agentChoice.model)}</select>
        <select class="mini" id="weffort" title="Reasoning effort">${effortOptionsHtml(providerCatalog.efforts, agentChoice.effort, selectedModel)}</select>
        <button class="btn" id="wclear">Clear</button><button class="btn primary" id="wrequest">Request Changes</button>`;
      q("#wprovider").onchange = (event) => {
        agentChoice.provider = event.target.value;
        agentChoice.model = "";
        agentChoice.effort = "";
        updateActions();
      };
      q("#wmodel").onchange = (event) => {
        agentChoice.model = event.target.value;
        agentChoice.effort = "";
        updateActions();
      };
      q("#weffort").onchange = (event) => {
        agentChoice.effort = event.target.value;
      };
      q("#wclear").onclick = () => {
        diffComments.length = 0;
        if (q("#wgeneral")) q("#wgeneral").value = "";
        applyHighlights();
        refreshFeedback();
      };
      q("#wrequest").onclick = async () => {
        const btn = q("#wrequest");
        btn.disabled = true;
        btn.textContent = "requesting…";
        const messages = diffThreadMessages(diffComments, q("#wgeneral") ? q("#wgeneral").value : "", null);
        try {
          await act(async () => {
            const selectedCatalog = catalogForProvider(agentCatalog, agentChoice.provider);
            adopting.setAdoptParams(
              modelParams(selectedCatalog.models, agentChoice.model, agentChoice.effort, agentChoice.provider),
            );
            await adopting.runCall("run.request_changes", { messages });
          });
          hideCommentPop();
          onAdopted();
        } catch (e) {
          if (adopting.adoptedRunId()) {
            // Adoption succeeded but the follow-up failed: the task now owns this
            // worktree and its error — hand off to it rather than stranding the
            // user on a surface whose polls are about to stop resolving.
            hideCommentPop();
            onAdopted();
            return;
          }
          if (!host) return;
          btn.disabled = false;
          btn.textContent = "Request Changes";
          notifyError("Request Changes failed", e.message);
        }
      };
      return;
    }
    // No pending comments: the finish-the-worktree affordances. A non-adoptable
    // worktree (detached HEAD, or the base branch itself) only browses.
    if (!meta || !meta.adoptable) {
      hint.textContent = meta && meta.branch
        ? "This is the base branch — check out a feature branch to adopt."
        : "Detached HEAD — check out a branch to adopt.";
      actions.innerHTML = "";
      return;
    }
    hint.textContent = "Comment on the diff to request changes, or finish the worktree.";
    const mergeHost = document.createElement("span");
    const abandon = document.createElement("button"); // quiet (plain) — the confirm guards it
    abandon.className = "btn";
    abandon.id = "wabandon";
    abandon.textContent = "Abandon & delete";
    actions.innerHTML = "";
    actions.appendChild(mergeHost);
    actions.appendChild(abandon);
    mountSplitButton(mergeHost, {
      options: WORKTREE_MERGE_OPTIONS,
      run: async (optionId) => {
        const { action, cleanup } = MERGE_RPC[optionId];
        // Confirm the exact step outline BEFORE adopting — a cancel leaves the
        // surface live and untouched.
        const confirmPlan = gitActionConfirm(optionId, { branch: branchLabel(), base: baseLabel() });
        if (confirmPlan && !(await confirmAction(confirmPlan))) throw new Error("cancelled");
        try {
          await act(() => adopting.runCall("run.git_action", { action, cleanup }));
          onFinished();
        } catch (e) {
          // A merge failure persists as an expandable notice (full message in
          // the detail).
          const reason = mergeFailureReason(e.message);
          notifyError(reason ? "Merge failed: " + reason.split("\n")[0] : "Action failed", e.message);
          if (adopting.adoptedRunId()) {
            // Adopted, then the merge failed (a conflict is the common case for a
            // stale external worktree): hand off to the task holding merge_failed.
            onAdopted();
            return;
          }
          throw e; // let the split button restore its primary
        }
      },
    });
    abandon.onclick = async () => {
      // adopted: true — this surface deletes files Build did not create.
      const confirmed = await confirmAction(abandonConfirm({ adopted: true, branch: branchLabel() }));
      if (!confirmed) return;
      abandon.disabled = true;
      abandon.textContent = "abandoning…";
      try {
        await act(() => adopting.runCall("run.abandon", {}));
        onFinished();
      } catch (e) {
        if (adopting.adoptedRunId()) {
          onAdopted();
          return;
        }
        if (!host) return;
        abandon.disabled = false;
        abandon.textContent = "Abandon & delete";
        notifyError("Abandon failed", e.message);
      }
    };
  }

  function renderBody(files) {
    const totalIns = files.reduce((a, f) => a + f.add, 0),
      totalDel = files.reduce((a, f) => a + f.del, 0);
    const editable = !!(meta && meta.adoptable);
    host.innerHTML = `
      <div class="diffbar"><span>${files.length} files <span style="color:var(--green)">+${totalIns}</span> <span style="color:var(--red)">−${totalDel}</span></span></div>
      ${files.length ? diffFilesHtml(files, { commentable: editable }) : '<div class="empty">No file changes yet.</div>'}
      ${editable ? `<div class="plan-feedback" id="wdiff-feedback"><div id="wdifflist"></div>
        <textarea id="wgeneral" class="plan-general" placeholder="Add a general comment about the changes and request updates…"></textarea></div>` : ""}
      <div class="actionbar"><span class="hint" id="wdiffhint"></span><div class="right" id="wdiffactions"></div></div>`;
    if (editable) {
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
        showCommentPop(sel.getRangeAt(0).getBoundingClientRect(), (comment) => addComment(file, a, b, sel.toString(), comment));
      });
      q("#wgeneral").oninput = updateActions;
    }
    // One delegated handler: the whole-file ✎ and line comments when adoptable,
    // secret spoilers always. Diff folding belongs to the git pane around us.
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
      if (!editable) return;
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
    applyHighlights();
    refreshFeedback();
  }

  const paint = async () => {
    if (!host || isOffline() || acting) return;
    // Adopted already? The worktree lives on as a task now — never poll it (the
    // diff would 404) — hand off so its outcome/error is where the user can see it.
    if (adopting.adoptedRunId()) {
      onAdopted();
      return;
    }
    let res;
    try {
      res = await callRpc("worktree.diff", { project_id: projectId, worktree_id: worktreeId });
    } catch (e) {
      if (String(e && e.message).includes("unknown worktree_id") && !acting) onGone();
      return; // otherwise transient — the poll retries
    }
    if (!host) return; // unmounted while the RPC was in flight
    meta = {
      branch: res.branch,
      base_branch: res.base_branch,
      path: res.path,
      adoptable: res.adoptable,
    };
    const files = filterNoiseFiles(parseDiff(res.patch));
    const key = String(res.adoptable) + " " + res.patch;
    const general = q("#wgeneral");
    // Freeze while the reviewer is mid-comment (pending comments, an open
    // popover, or text in the general box) so anchors and selection survive —
    // and skip the rebuild when nothing changed (fold state survives too).
    const busy =
      diffComments.length > 0 ||
      hasCommentPop() ||
      (general && (general.value.trim() || document.activeElement === general));
    if (q(".diffbar") && (key === diffKey || busy)) {
      updateActions();
      return;
    }
    diffKey = key;
    renderBody(files);
  };

  return {
    /** The branch the rail's "All changes" entry names this diff against. */
    getBase: () => baseLabel(),
    mount(el) {
      host = el;
      diffKey = null; // a fresh host always needs a first paint
      host.innerHTML = '<div class="empty">loading…</div>';
      paint();
      timer = setInterval(whenVisible(paint), WORKTREE_REVIEW_POLL_MS);
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
