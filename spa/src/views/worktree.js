// The read-only external-worktree browse view: shows a worktree's dirty diff
// (merge-base anchored) without adopting it. The first mutating action — Request
// Changes, a Merge variant, or Abandon — transparently adopts the worktree as a
// task (createAdoptingCall) and then proceeds. Everything rendered from the
// worktree's branch, subject, path, and diff is UNTRUSTED and escaped.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { parseDiff, filterNoiseFiles } from "../core/diff.js";
import { diffFilesHtml } from "../core/diffRender.js";
import { assembleDiffNotes } from "../core/notes.js";
import { App, go } from "../app.js";
import { mountSplitButton } from "../core/splitButton.js";
import { createAdoptingCall } from "../core/adoption.js";
import { showCommentPop, hideCommentPop, hasCommentPop } from "../commentPop.js";
import { watchSelection } from "../selectWatch.js";
import { mountTabShell } from "../core/tabshell.js";
import { terminalTabsController, mountAuxTab } from "../core/surfaceTabs.js";

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
  const scope = { project_id: projectId, worktree_id: worktreeId };

  // Review-comment state, preserved across the 1.6s poll (same discipline as the
  // task diff tab). Comments only accrue on an adoptable worktree.
  const diffComments = []; // { id, file, lnA, lnB, snippet, comment }
  let dcid = 0,
    diffKey = null,
    diffSelDispose = null,
    shellState = null;

  // The unified tab shell: Diff (the entire existing review content, untouched),
  // Files, then user terminal tabs with `+`. Default tab: diff. Scope:
  // { project_id, worktree_id }.
  let tab = App.route.tab || "diff";
  const terminals = terminalTabsController(scope);
  let shellCtl = null;
  let aux = null;
  const isAuxTab = (tabId) => tabId === "files" || /^term-/.test(tabId);
  const staticTabs = () => [{ id: "diff", label: "Diff" }, { id: "files", label: "Files" }, ...terminals.tabs()];
  const disposeAux = () => {
    if (aux) {
      aux.dispose();
      aux = null;
    }
  };

  const selectTab = (tabId) => {
    tab = tabId;
    App.route.tab = tabId;
    history.replaceState(null, "", `#/worktree/${encodeURIComponent(projectId)}/${encodeURIComponent(worktreeId)}/${tabId}`);
    if (shellCtl) shellCtl.setActive(tabId);
    disposeAux();
    if (tabId === "diff") {
      const body = $("#tabbody");
      if (body) body.classList.remove("bare", "flush");
      diffKey = null;
      shellState = null; // force a shell rebuild + body repaint on the next paint
      paint();
    } else {
      mountAux(tabId);
    }
  };

  const mountAux = (tabId) => {
    disposeAux();
    const body = $("#tabbody");
    if (!body) return;
    // Terminal tabs go edge-to-edge; the Files browser runs flush (tree rail +
    // preview pane each scroll internally, so the body owns no padding).
    body.classList.toggle("bare", /^term-/.test(tabId));
    body.classList.toggle("flush", tabId === "files");
    aux = mountAuxTab(body, tabId, {
      scope,
      callRpc: (method, params) => App.call(method, params),
      onExit: () => {
        terminals.drop(tabId);
        if (shellCtl) shellCtl.setTabs(staticTabs());
        selectTab("diff");
      },
    });
  };

  const newTerminal = async () => {
    let termId;
    try {
      termId = await terminals.create();
    } catch (e) {
      showError("cannot open a terminal: " + e.message.slice(0, 80));
      return;
    }
    if (shellCtl) shellCtl.setTabs(staticTabs());
    selectTab(termId);
  };

  const closeTerminal = async (termId) => {
    try {
      await terminals.close(termId);
    } catch {
      /* raced with the reaper — drop the tab regardless */
    }
    if (shellCtl) shellCtl.setTabs(staticTabs());
    if (tab === termId) selectTab("diff");
  };

  const goHome = () => go({ name: "project", projectId });

  const renderNotFound = () => {
    root.innerHTML = `
      <div class="back" id="back">← Project</div>
      <div class="empty">This worktree is no longer available — it may have been adopted or removed.</div>`;
    $("#back").onclick = () => goHome();
  };

  const stopPolling = () => {
    if (App.poll) {
      clearInterval(App.poll);
      App.poll = null;
    }
  };
  const startPolling = () => {
    if (!App.poll) App.poll = setInterval(paint, 1600);
  };
  // Once adoption has succeeded this worktree is bound to a task, so worktree.diff
  // (and the poll) will report "unknown worktree_id" — that is the EXPECTED
  // post-adoption state, not "the worktree vanished". Hand off to the freshly
  // minted task (which now holds any merge_failed reason) instead of wiping the
  // view with the "no longer available" empty state.
  const handoffToTask = () => {
    stopPolling();
    go({ name: "task", id: adopting.adoptedRunId(), tab: "changes" });
  };

  // The tab bar is the top of the view; the worktree's identity (branch) rides
  // the bar's right cluster, path on hover.
  const shell = (meta) => {
    root.innerHTML = `
      <div class="surface-bar">
        <div class="tabrow" id="tabrow"></div>
        <div class="surface-meta">
          <span class="mono dim" title="${esc(meta.path || "")}">${esc(meta.branch || "(detached)")}</span>
          <span class="chip" title="Read-only — acting on this worktree adopts it as a task.">WORKTREE</span>
        </div>
      </div>
      <div class="task-error" id="wtError" role="alert" hidden></div>
      <div id="tabbody"></div>`;
    shellCtl = mountTabShell($("#tabrow"), {
      tabs: staticTabs(),
      active: tab,
      onSelect: (tabId) => selectTab(tabId),
      onClose: (tabId) => closeTerminal(tabId),
      onNewTerminal: () => newTerminal(),
      back: { title: "Back to project" },
      onBack: () => goHome(),
    });
    // A shell rebuild wiped #tabbody — re-mount an aux tab so the poll's
    // early-return leaves a live Files/terminal pane in place.
    if (isAuxTab(tab)) mountAux(tab);
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
          const location = c.lnA === 0 && c.lnB === 0 ? "" : c.lnA === c.lnB ? `:${c.lnA}` : `:${c.lnA}-${c.lnB}`;
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
        // Suspend the poll: this action adopts the worktree (binding it to a task),
        // after which the poll's worktree.diff would resolve to "unknown
        // worktree_id" and race us to renderNotFound.
        stopPolling();
        const notes = assembleDiffNotes(diffComments, $("#wgeneral") ? $("#wgeneral").value : "");
        try {
          await adopting.runCall("run.request_changes", { comments: notes });
          hideCommentPop();
          go({ name: "task", id: adopting.adoptedRunId(), tab: "changes" });
        } catch (e) {
          if (adopting.adoptedRunId()) {
            // Adoption succeeded but the follow-up failed: the task now owns this
            // worktree and its error — hand off to it rather than stranding the
            // user on a route the poll is about to blank.
            hideCommentPop();
            handoffToTask();
            return;
          }
          btn.disabled = false;
          btn.textContent = "Request Changes";
          showError("error: " + e.message.slice(0, 80));
          startPolling();
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
        stopPolling(); // adoption binds the worktree; the poll must not race us
        try {
          await adopting.runCall("run.git_action", { action, cleanup });
          goHome();
        } catch (e) {
          if (adopting.adoptedRunId()) {
            // Adopted, then the merge failed (a conflict is the common case for a
            // stale external worktree): hand off to the task holding merge_failed.
            handoffToTask();
            return;
          }
          showError("error: " + e.message.slice(0, 80));
          startPolling();
          throw e;
        }
      },
    });
    abandon.onclick = async () => {
      if (!window.confirm("Delete this worktree and its branch? This removes files Build did not create.")) return;
      abandon.disabled = true;
      abandon.textContent = "abandoning…";
      stopPolling(); // adoption binds the worktree; the poll must not race us
      try {
        await adopting.runCall("run.abandon", {});
        goHome();
      } catch (e) {
        if (adopting.adoptedRunId()) {
          handoffToTask();
          return;
        }
        abandon.disabled = false;
        abandon.textContent = "Abandon & delete";
        showError("error: " + e.message.slice(0, 80));
        startPolling();
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
      ${files.length ? diffFilesHtml(files, { commentable: editable }) : '<div class="empty">No file changes yet.</div>'}
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
      $("#wgeneral").oninput = updateActions;
    }
    // One delegated handler: diff folding always (capped body expands, the
    // filename bar toggles collapse), commenting only when adoptable.
    body.onclick = (e) => {
      const commentButton = e.target.closest(".fcmt");
      if (commentButton) {
        const fileEl = commentButton.closest(".file");
        if (fileEl)
          showCommentPop(commentButton.getBoundingClientRect(), (comment) =>
            addComment(fileEl.dataset.file, 0, 0, "(entire file)", comment),
          );
        return;
      }
      const fhead = e.target.closest(".fhead");
      if (fhead && !e.target.closest("button, input, label")) {
        const file = fhead.closest(".file");
        if (file) {
          file.classList.toggle("collapsed");
          file.classList.remove("capped");
        }
        return;
      }
      const capped = e.target.closest(".file.capped");
      if (capped) {
        capped.classList.remove("capped");
        return;
      }
      if (!editable) return;
      const sel = window.getSelection();
      if (sel && !sel.isCollapsed && sel.toString().trim()) return; // range flow owns it
      const fileEl = e.target.closest(".file");
      const tr = e.target.closest("tr[data-ln]");
      if (!fileEl || !tr || tr.classList.contains("hunk") || !tr.dataset.ln) return;
      const ln = +tr.dataset.ln,
        snippet = tr.querySelector(".code").textContent;
      showCommentPop(tr.getBoundingClientRect(), (comment) => addComment(fileEl.dataset.file, ln, ln, snippet, comment));
    };
    applyHighlights();
    refreshFeedback();
  };

  const paint = async () => {
    if (App.offline) return;
    // Adopted already? The worktree lives on as a task now — never poll it (the
    // diff would 404) — hand off so its outcome/error is where the user can see it.
    if (adopting.adoptedRunId()) {
      handoffToTask();
      return;
    }
    let res;
    try {
      res = await App.call("worktree.diff", { project_id: projectId, worktree_id: worktreeId });
    } catch (e) {
      if (String(e && e.message).includes("unknown worktree_id")) {
        // Bound to a task since the last poll → hand off; otherwise it was
        // genuinely removed and the not-found state is correct.
        if (adopting.adoptedRunId()) {
          handoffToTask();
        } else {
          stopPolling();
          renderNotFound();
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
    const files = filterNoiseFiles(parseDiff(res.patch));
    const key = String(res.adoptable) + " " + res.patch;
    const general = $("#wgeneral");
    const busy =
      diffComments.length > 0 || hasCommentPop() || (general && (general.value.trim() || document.activeElement === general));
    // The shell rebuild wipes #tabbody (the in-progress general comment + its
    // focus live only there), so it obeys the SAME freeze-while-commenting
    // discipline as the body repaint below — never rebuild while the reviewer
    // is mid-comment. Only the fields the bar actually shows are keyed; the
    // churny ones (dirty_files, head_subject) no longer render anywhere.
    const shellKey = `${meta.branch}|${meta.adoptable}|${meta.path}`;
    // Rebuild the header on first paint always; afterward only while the Diff tab
    // is active — an aux tab (Files/terminal) owns #tabbody and must not be wiped
    // by a churny header refresh (dirty_files/head_subject move as the user edits
    // their own live checkout). Switching back to Diff resets shellState.
    if (!busy && (shellState === null || (tab === "diff" && shellState.key !== shellKey))) {
      shell(meta);
      shellState = { ...meta, key: shellKey };
      diffKey = null; // shell wiped #tabbody — force a body repaint below
    }
    // Files and terminal tabs are fetch-/push-driven — the poll keeps only the
    // header current (and watches for the worktree vanishing) for them.
    if (tab !== "diff") return;
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

  // Tear down any mounted terminal/files pane when navigating away.
  App.viewDispose = () => disposeAux();

  await terminals.load();
  await paint();
  App.poll = setInterval(paint, 1600);
}
