// The read-only external-worktree browse view: shows a worktree's dirty diff
// (merge-base anchored) without adopting it. The first mutating action — Request
// Changes, a Merge variant, or Abandon — transparently adopts the worktree as a
// task (createAdoptingCall) and then proceeds. Everything rendered from the
// worktree's branch, subject, path, and diff is UNTRUSTED and escaped.

import { $ } from "../dom.js";
import { esc, humanAge } from "../core/text.js";
import { parseDiff, filterNoiseFiles } from "../core/diff.js";
import { diffFilesHtml } from "../core/diffRender.js";
import { assembleDiffNotes } from "../core/notes.js";
import { App, go } from "../app.js";
import { mountSplitButton } from "../core/splitButton.js";
import { createAdoptingCall } from "../core/adoption.js";
import { showCommentPop, hideCommentPop, hasCommentPop } from "../commentPop.js";
import { watchSelection } from "../selectWatch.js";

// The adopted-task merge set for the browse view: prune / keep / release only
// (no commit/push/merge_push here — those belong to a task already in review).
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

export async function renderWorktree() {
  const root = $("#root");
  const projectId = App.route.projectId;
  const worktreeId = App.route.worktreeId;
  const adopting = createAdoptingCall((method, params) => App.call(method, params), projectId, worktreeId);

  // Review-comment state, preserved across the 1.6s poll (same discipline as the
  // task diff tab). Comments only accrue on an adoptable worktree.
  const diffComments = []; // { id, file, lnA, lnB, snippet, comment }
  let dcid = 0,
    diffKey = null,
    diffSelDispose = null,
    shellState = null;

  const renderNotFound = () => {
    root.innerHTML = `
      <div class="back" id="back">← Board</div>
      <div class="empty">This worktree is no longer available — it may have been adopted or removed.</div>`;
    $("#back").onclick = () => go({ name: "board" });
  };

  const shell = (meta) => {
    const uncommitted = meta.dirty_files ? ` · ${meta.dirty_files} uncommitted` : "";
    root.innerHTML = `
      <div class="back" id="back">← Board</div>
      <div class="thead"><h1>${esc(meta.branch || "(detached)")}</h1>
        <div class="right"><span class="chip">WORKTREE</span></div></div>
      <div class="tmeta"><span>${esc(meta.head_subject || "")}</span><span>·</span><span>${esc(meta.path || "")}</span>${uncommitted ? `<span>·</span><span>${esc(meta.dirty_files + " uncommitted")}</span>` : ""}</div>
      <div class="tis">Read-only — acting on this worktree adopts it as a task.</div>
      <div class="task-error" id="wtError" role="alert" hidden></div>
      <div id="tabbody"></div>`;
    $("#back").onclick = () => go({ name: "board" });
  };

  const showError = (message) => {
    const el = $("#wtError");
    if (!el) return;
    el.textContent = message || "";
    el.hidden = !message;
  };

  // The <tr> (with a line number) containing a selection/click node.
  const rowOf = (node, table) => {
    let element = node && node.nodeType === 3 ? node.parentElement : node;
    while (element && element !== table && element.tagName !== "TR") element = element.parentElement;
    return element && element.tagName === "TR" && element.dataset.ln ? element : null;
  };

  const applyHighlights = () => {
    const body = $("#tabbody");
    if (!body) return;
    body.querySelectorAll("tr.dhl").forEach((r) => r.classList.remove("dhl"));
    diffComments.forEach((c) => {
      const fileEl = Array.from(body.querySelectorAll(".file")).find((element) => element.dataset.file === c.file);
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

  const refreshFeedback = () => {
    const list = $("#wdifflist");
    if (list) {
      list.innerHTML = diffComments
        .map((c) => {
          const location = c.lnA === c.lnB ? `:${c.lnA}` : `:${c.lnA}-${c.lnB}`;
          return `<div class="pcomment"><span class="pcx" data-id="${c.id}">×</span>
            <span class="psnip">${esc(c.file)}${esc(location)} · ${esc(c.snippet.replace(/\s+/g, " ").trim().slice(0, 90))}</span>
            <span class="pctext">${esc(c.comment)}</span></div>`;
        })
        .join("");
      list.querySelectorAll(".pcx").forEach((x) => (x.onclick = () => removeComment(+x.dataset.id)));
    }
    updateActions();
  };

  const updateActions = () => {
    const actions = $("#wdiffactions"),
      hint = $("#wdiffhint");
    if (!actions) return;
    const general = $("#wgeneral") ? $("#wgeneral").value.trim() : "";
    if (diffComments.length || general) {
      hint.textContent = "Your comments adopt this worktree as a task and are sent to the coding agent.";
      actions.innerHTML = `<button class="btn" id="wclear">Clear</button><button class="btn primary" id="wrequest">Request Changes</button>`;
      $("#wclear").onclick = () => {
        diffComments.length = 0;
        if ($("#wgeneral")) $("#wgeneral").value = "";
        applyHighlights();
        refreshFeedback();
      };
      $("#wrequest").onclick = async () => {
        const btn = $("#wrequest");
        btn.disabled = true;
        btn.textContent = "requesting…";
        const notes = assembleDiffNotes(diffComments, $("#wgeneral") ? $("#wgeneral").value : "");
        try {
          await adopting.taskCall("task.request_changes", { comments: notes });
          hideCommentPop();
          go({ name: "task", id: adopting.adoptedTaskId(), tab: "diff" });
        } catch (e) {
          btn.disabled = false;
          btn.textContent = "Request Changes";
          showError("error: " + e.message.slice(0, 80));
        }
      };
      return;
    }
    // No pending comments: the finish-the-worktree affordances. A non-adoptable
    // worktree (detached HEAD, or the base branch itself) only browses.
    if (!shellState || !shellState.adoptable) {
      hint.textContent = shellState && shellState.branch
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
        try {
          await adopting.taskCall("task.git_action", { action, cleanup });
          go({ name: "board" });
        } catch (e) {
          showError("error: " + e.message.slice(0, 80));
          throw e;
        }
      },
    });
    abandon.onclick = async () => {
      if (!window.confirm("Delete this worktree and its branch? This removes files Build did not create.")) return;
      abandon.disabled = true;
      abandon.textContent = "abandoning…";
      try {
        await adopting.taskCall("task.abandon", {});
        go({ name: "board" });
      } catch (e) {
        abandon.disabled = false;
        abandon.textContent = "Abandon & delete";
        showError("error: " + e.message.slice(0, 80));
      }
    };
  };

  const renderBody = (meta, files) => {
    const body = $("#tabbody");
    const totalIns = files.reduce((a, f) => a + f.add, 0),
      totalDel = files.reduce((a, f) => a + f.del, 0);
    const editable = !!meta.adoptable;
    body.innerHTML = `
      <div class="diffbar"><span>${files.length} files <span style="color:var(--green)">+${totalIns}</span> <span style="color:var(--red)">−${totalDel}</span></span></div>
      ${files.length ? diffFilesHtml(files) : '<div class="empty">No file changes yet.</div>'}
      ${editable ? `<div class="plan-feedback" id="wdiff-feedback"><div id="wdifflist"></div>
        <textarea id="wgeneral" class="plan-general" placeholder="Add a general comment about the changes and request updates…"></textarea></div>` : ""}
      <div class="actionbar"><span class="hint" id="wdiffhint"></span><div class="right" id="wdiffactions"></div></div>`;
    if (editable) {
      if (diffSelDispose) diffSelDispose();
      diffSelDispose = watchSelection(body, (sel) => {
        const fileEl = rowOf(sel.anchorNode, body)?.closest(".file");
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
      body.onclick = (e) => {
        const sel = window.getSelection();
        if (sel && !sel.isCollapsed && sel.toString().trim()) return; // range flow owns it
        const fileEl = e.target.closest(".file");
        const tr = e.target.closest("tr[data-ln]");
        if (!fileEl || !tr || tr.classList.contains("hunk") || !tr.dataset.ln) return;
        const ln = +tr.dataset.ln,
          snippet = tr.querySelector(".code").textContent;
        showCommentPop(tr.getBoundingClientRect(), (comment) => addComment(fileEl.dataset.file, ln, ln, snippet, comment));
      };
      $("#wgeneral").oninput = updateActions;
    }
    applyHighlights();
    refreshFeedback();
  };

  const paint = async () => {
    if (App.offline) return;
    let res;
    try {
      res = await App.call("worktree.diff", { project_id: projectId, worktree_id: worktreeId });
    } catch (e) {
      if (String(e && e.message).includes("unknown worktree_id")) {
        renderNotFound();
        if (App.poll) {
          clearInterval(App.poll);
          App.poll = null;
        }
      }
      return; // transient — the poll retries
    }
    const meta = {
      branch: res.branch,
      head_subject: res.head_subject,
      path: res.path,
      dirty_files: res.dirty_files,
      adoptable: res.adoptable,
    };
    const shellKey = `${meta.branch}|${meta.adoptable}|${meta.dirty_files}|${meta.head_subject}|${meta.path}`;
    if (shellState === null || shellState.key !== shellKey) {
      shell(meta);
      shellState = { ...meta, key: shellKey };
      diffKey = null; // shell wiped #tabbody — force a body repaint below
    }
    const files = filterNoiseFiles(parseDiff(res.patch));
    const key = String(res.adoptable) + " " + res.patch;
    const general = $("#wgeneral");
    const busy =
      diffComments.length > 0 || hasCommentPop() || (general && (general.value.trim() || document.activeElement === general));
    if ($("#wdiff-feedback") && (key === diffKey || busy)) {
      updateActions();
      return;
    }
    // A non-adoptable worktree has no feedback box; still repaint when the patch
    // changes (no comment state can be frozen there).
    if (!meta.adoptable && $("#tabbody") && key === diffKey) return;
    diffKey = key;
    renderBody(meta, files);
  };

  await paint();
  App.poll = setInterval(paint, 1600);
}
