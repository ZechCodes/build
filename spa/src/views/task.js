// The task view: plan tab (select-to-comment review) + diff tab (line comments,
// request-changes, and the git split button), live-polled every 1.6s.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { renderMarkdown } from "../core/markdown.js";
import { parseDiff, filterNoiseFiles } from "../core/diff.js";
import { assemblePlanNotes, assembleDiffNotes } from "../core/notes.js";
import { App, go, loadModelCatalog } from "../app.js";
import { modelOptionsHtml, effortOptionsHtml, effortSupported, modelParams } from "../core/modelPicker.js";
import { STATE_LABEL, chipClass } from "./shared.js";
import { showCommentPop, hideCommentPop, hasCommentPop } from "../commentPop.js";

export async function renderTask() {
  const root = $("#root");
  const id = App.route.id;
  let tab = App.route.tab || "plan";
  const shell = (t) => {
    const m = t || {};
    root.innerHTML = `
      <div class="back" id="back">← Board</div>
      <div class="thead"><h1>${esc(m.goal || "")}</h1>
        <div class="right"><span class="chip ${chipClass(m.state)}">${STATE_LABEL[m.state] || m.state || ""}</span>
          <span>terminal <span class="kbd">\`</span></span></div></div>
      <div class="tmeta"><span>${esc(m.project || "")}</span><span>·</span><span>${esc(m.branch || "")}</span><span>·</span><span>${esc(m.harness || "")}</span></div>
      <div class="tabs"><div class="t ${tab === "plan" ? "active" : ""}" data-tab="plan">Plan</div>
        <div class="t ${tab === "diff" ? "active" : ""}" data-tab="diff">Diff</div></div>
      <div id="tabbody"></div>`;
    $("#back").onclick = () => go({ name: "board" });
    root.querySelectorAll(".tabs .t").forEach(
      (e) =>
        (e.onclick = () => {
          tab = e.dataset.tab;
          App.route.tab = tab;
          history.replaceState(null, "", `#/task/${encodeURIComponent(id)}/${tab}`);
          root.querySelectorAll(".tabs .t").forEach((x) => x.classList.toggle("active", x.dataset.tab === tab));
          paint();
        }),
    );
  };
  let last = null;
  loadModelCatalog(); // warm the selector catalog before plan_review needs it
  // Plan-review feedback state, preserved across the 1.6s poll.
  const planComments = []; // { id, snippet, comment }
  let cid = 0,
    planKey = null;

  // Build the plan tab: rendered markdown + select-to-comment + a general
  // comment box + an action button that morphs to "Request Updates".
  function renderPlanTab(t, plan) {
    planComments.length = 0; // a freshly (re)rendered plan starts with no comments
    const editable = t.state === "plan_review";
    const body = $("#tabbody");
    body.innerHTML = `
      <div class="plan" id="planbody">${renderMarkdown(plan)}</div>
      ${editable ? `<div class="plan-feedback"><div id="pclist"></div>
        <textarea id="pgeneral" class="plan-general" placeholder="Add a general comment about the plan and request updates…"></textarea></div>` : ""}
      <div class="actionbar"><span class="hint" id="phint"></span><div class="right" id="pactions"></div></div>`;
    const pclist = $("#pclist"),
      pactions = $("#pactions"),
      phint = $("#phint");

    const removeComment = (idc) => {
      const i = planComments.findIndex((c) => c.id === idc);
      if (i >= 0) planComments.splice(i, 1);
      const mark = document.querySelector(`mark.phl[data-cid="${idc}"]`);
      if (mark) {
        const parent = mark.parentNode;
        while (mark.firstChild) parent.insertBefore(mark.firstChild, mark);
        parent.removeChild(mark);
        parent.normalize();
      }
      refreshFeedback();
    };
    const updateActions = () => {
      const general = editable && $("#pgeneral") ? $("#pgeneral").value.trim() : "";
      if (!editable) {
        pactions.innerHTML = "";
        phint.textContent = "";
        return;
      }
      if (planComments.length || general) {
        phint.textContent = "Your comments will be sent to the planning agent to revise the plan.";
        pactions.innerHTML = `<button class="btn" id="clearfb">Clear</button><button class="btn primary" id="requestUpdates">Request Updates</button>`;
        $("#clearfb").onclick = () => {
          planComments.slice().forEach((c) => removeComment(c.id));
          if ($("#pgeneral")) $("#pgeneral").value = "";
          refreshFeedback();
        };
        $("#requestUpdates").onclick = async () => {
          const btn = $("#requestUpdates");
          btn.disabled = true;
          btn.textContent = "requesting updates…";
          const notes = assemblePlanNotes(planComments, $("#pgeneral") ? $("#pgeneral").value : "");
          try {
            await App.call("task.send_notes", { task_id: id, comments: notes });
            planComments.length = 0;
            planKey = null;
            hideCommentPop();
            paint();
          } catch (e) {
            btn.disabled = false;
            btn.textContent = "Request Updates";
            phint.textContent = "error: " + e.message.slice(0, 50);
          }
        };
      } else {
        phint.textContent = "Select text in the plan to comment, or approve to start the build.";
        // The coding agent's model defaults to the task's dispatch-time choice;
        // picking here overrides it for the build (and later revisions).
        const catalog = App.modelCatalog || { models: [], efforts: [] };
        pactions.innerHTML = `
          <select id="apModel" class="mini" title="Coding agent model">${modelOptionsHtml(catalog.models, t.model)}</select>
          <select id="apEffort" class="mini" title="Reasoning effort">${effortOptionsHtml(catalog.efforts, t.effort)}</select>
          <button class="btn primary" id="approvePlan">Approve plan &amp; start build</button>`;
        const syncEffort = () => {
          const supported = effortSupported(catalog.models, $("#apModel").value);
          $("#apEffort").disabled = !supported;
          if (!supported) $("#apEffort").value = "";
        };
        $("#apModel").onchange = syncEffort;
        syncEffort();
        $("#approvePlan").onclick = async () => {
          const approve = $("#approvePlan");
          approve.disabled = true;
          approve.textContent = "starting build…";
          const params = modelParams(catalog.models, $("#apModel").value, $("#apEffort").value);
          try {
            await App.call("task.approve_plan", { task_id: id, ...params });
          } catch (e) {
            // Restore the button — the poll's key-diffing skips repaints when
            // nothing changed, so a wedged button would otherwise stay dead.
            approve.disabled = false;
            approve.textContent = "Approve plan & start build";
            phint.textContent = "error: " + e.message.slice(0, 50);
            return;
          }
          App.route.tab = "diff";
          tab = "diff";
          planKey = null;
          paint();
        };
      }
    };
    function refreshFeedback() {
      if (pclist) {
        pclist.innerHTML = planComments
          .map(
            (c) => `
          <div class="pcomment"><span class="pcx" data-id="${c.id}">×</span>
            <span class="psnip">${esc(c.snippet.replace(/\s+/g, " ").trim().slice(0, 160))}</span>
            <span class="pctext">${esc(c.comment)}</span></div>`,
          )
          .join("");
        pclist.querySelectorAll(".pcx").forEach((x) => (x.onclick = () => removeComment(+x.dataset.id)));
      }
      updateActions();
    }
    const addComment = (snippet, comment, range) => {
      const idc = ++cid;
      planComments.push({ id: idc, snippet, comment });
      try {
        const mark = document.createElement("mark");
        mark.className = "phl";
        mark.dataset.cid = idc;
        range.surroundContents(mark);
      } catch {
        /* selection spanned nodes — keep the comment without the highlight */
      }
      window.getSelection().removeAllRanges();
      refreshFeedback();
    };
    if (editable) {
      const planEl = $("#planbody");
      planEl.addEventListener("mouseup", () =>
        setTimeout(() => {
          const sel = window.getSelection();
          const text = sel.toString().trim();
          if (!text || sel.rangeCount === 0 || !planEl.contains(sel.anchorNode) || !planEl.contains(sel.focusNode)) return;
          const range = sel.getRangeAt(0).cloneRange();
          showCommentPop(range.getBoundingClientRect(), (comment) => addComment(text, comment, range));
        }, 0),
      );
      $("#pgeneral").oninput = updateActions;
    }
    refreshFeedback();
  }

  // Diff-review feedback state, preserved across the live-updating diff poll.
  const diffComments = []; // { id, file, lnA, lnB, snippet, comment }
  let dcid = 0,
    diffKey = null,
    lastDiffState = null,
    diffMsg = "";

  // The <tr> (with a line number) containing a selection/click node.
  const rowOf = (node, table) => {
    let element = node && node.nodeType === 3 ? node.parentElement : node;
    while (element && element !== table && element.tagName !== "TR") element = element.parentElement;
    return element && element.tagName === "TR" && element.dataset.ln ? element : null;
  };

  function renderDiffTab(t, files) {
    const editable = t.state === "review" || t.state === "building";
    const working = t.state === "building";
    const totalIns = files.reduce((a, f) => a + f.add, 0),
      totalDel = files.reduce((a, f) => a + f.del, 0);
    const body = $("#tabbody");
    const fileHtml = files
      .map(
        (f) => `
      <div class="file" data-file="${esc(f.path)}"><div class="fhead"><span>${esc(f.path)}</span><span class="fb ${f.status}">${f.status}</span>
        <span class="pm"><span class="a">+${f.add}</span> <span class="d">−${f.del}</span></span></div>
        <table>${f.rows
          .map((r) =>
            r.t === "hunk"
              ? `<tr class="hunk"><td class="ln"></td><td class="ln"></td><td class="code">${esc(r.text)}</td></tr>`
              : `<tr class="${r.t}" data-ln="${r.n ?? r.o ?? ""}"><td class="ln">${r.o ?? ""}</td><td class="ln">${r.n ?? ""}</td><td class="code">${esc(r.text)}</td></tr>`,
          )
          .join("")}</table></div>`,
      )
      .join("");
    body.innerHTML = `
      <div class="diffbar"><span>${files.length} files <span style="color:var(--green)">+${totalIns}</span> <span style="color:var(--red)">−${totalDel}</span></span>
        ${working ? '<span class="dim">● coding agent working — diff updating live…</span>' : ""}</div>
      ${files.length ? fileHtml : '<div class="empty">No file changes yet.</div>'}
      ${editable ? `<div class="plan-feedback" id="diff-feedback"><div id="difflist"></div>
        <textarea id="dgeneral" class="plan-general" placeholder="Add a general comment about the changes and request updates…"></textarea></div>` : ""}
      <div class="actionbar"><span class="hint" id="diffhint"></span><div class="right" id="diffactions"></div></div>`;

    if (editable) {
      body.querySelectorAll(".file").forEach((fileEl) => {
        const file = fileEl.dataset.file,
          table = fileEl.querySelector("table");
        if (!table) return;
        table.addEventListener("mouseup", () =>
          setTimeout(() => {
            const sel = window.getSelection();
            if (sel.rangeCount === 0) return;
            const text = sel.toString();
            if (text.trim()) {
              const startRow = rowOf(sel.anchorNode, table),
                endRow = rowOf(sel.focusNode, table);
              if (!startRow && !endRow) return;
              let a = +(startRow || endRow).dataset.ln,
                b = +(endRow || startRow).dataset.ln;
              if (a > b) [a, b] = [b, a];
              showCommentPop(sel.getRangeAt(0).getBoundingClientRect(), (comment) => addDiffComment(file, a, b, text, comment));
            } else {
              const tr = rowOf(sel.anchorNode, table);
              if (!tr || tr.classList.contains("hunk")) return;
              const ln = +tr.dataset.ln,
                snippet = tr.querySelector(".code").textContent;
              showCommentPop(tr.getBoundingClientRect(), (comment) => addDiffComment(file, ln, ln, snippet, comment));
            }
          }, 0),
        );
      });
      $("#dgeneral").oninput = updateDiffActions;
    }
    applyDiffHighlights();
    refreshDiffFeedback();
  }

  function applyDiffHighlights() {
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
  }
  const addDiffComment = (file, lnA, lnB, snippet, comment) => {
    const idc = ++dcid;
    diffComments.push({ id: idc, file, lnA: lnA || lnB, lnB: lnB || lnA, snippet: snippet.trim().slice(0, 400), comment });
    window.getSelection().removeAllRanges();
    applyDiffHighlights();
    refreshDiffFeedback();
  };
  const removeDiffComment = (idc) => {
    const i = diffComments.findIndex((c) => c.id === idc);
    if (i >= 0) diffComments.splice(i, 1);
    applyDiffHighlights();
    refreshDiffFeedback();
  };
  function refreshDiffFeedback() {
    const list = $("#difflist");
    if (list) {
      list.innerHTML = diffComments
        .map((c) => {
          const location = c.lnA === c.lnB ? `:${c.lnA}` : `:${c.lnA}-${c.lnB}`;
          return `<div class="pcomment"><span class="pcx" data-id="${c.id}">×</span>
            <span class="psnip">${esc(c.file)}${esc(location)} · ${esc(c.snippet.replace(/\s+/g, " ").trim().slice(0, 90))}</span>
            <span class="pctext">${esc(c.comment)}</span></div>`;
        })
        .join("");
      list.querySelectorAll(".pcx").forEach((x) => (x.onclick = () => removeDiffComment(+x.dataset.id)));
    }
    updateDiffActions();
  }
  function updateDiffActions() {
    const actions = $("#diffactions"),
      hint = $("#diffhint");
    if (!actions) return;
    const general = $("#dgeneral") ? $("#dgeneral").value.trim() : "";
    if (diffComments.length || general) {
      hint.textContent = "Your comments will be sent to the coding agent to make changes.";
      actions.innerHTML = `<button class="btn" id="clearrc">Clear</button><button class="btn primary" id="requestChanges">Request Changes</button>`;
      $("#clearrc").onclick = () => {
        diffComments.length = 0;
        if ($("#dgeneral")) $("#dgeneral").value = "";
        applyDiffHighlights();
        refreshDiffFeedback();
      };
      $("#requestChanges").onclick = async () => {
        const btn = $("#requestChanges");
        btn.disabled = true;
        btn.textContent = "requesting…";
        const notes = assembleDiffNotes(diffComments, $("#dgeneral") ? $("#dgeneral").value : "");
        try {
          await App.call("task.request_changes", { task_id: id, comments: notes });
          diffComments.length = 0;
          if ($("#dgeneral")) $("#dgeneral").value = "";
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
      // GitHub-style split button: primary runs the default (merge), the caret
      // opens the full menu. Every action commits first; push is explicit.
      actions.innerHTML = `
        <div class="splitbtn">
          <button class="btn primary" id="gitprimary" data-action="merge">Merge</button>
          <button class="btn primary caret" id="gitcaret" title="More actions">▾</button>
          <div class="splitmenu" id="gitmenu" hidden>
            <div class="mi" data-action="commit"><span class="mt">Commit</span><span class="md">commit the work, stay on the branch</span></div>
            <div class="mi" data-action="merge"><span class="mt">Merge</span><span class="md">commit, then merge into ${esc((last && last.base_branch) || "main")}</span></div>
            <div class="mi" data-action="merge_push"><span class="mt">Merge &amp; push</span><span class="md">merge, then push ${esc((last && last.base_branch) || "main")} to origin</span></div>
            <div class="mi" data-action="push"><span class="mt">Push</span><span class="md">commit, then push this branch to origin</span></div>
          </div>
        </div>`;
      const labels = { commit: "committing…", merge: "merging…", merge_push: "merging & pushing…", push: "pushing…" };
      const flash = (msg) => {
        diffMsg = msg;
        setTimeout(() => {
          diffMsg = "";
          updateDiffActions();
        }, 6000);
      };
      const run = async (action, btn) => {
        btn.disabled = true;
        diffMsg = "";
        const originalLabel = btn.textContent;
        btn.textContent = labels[action] || "working…";
        try {
          await App.call("task.git_action", { task_id: id, action });
          if (action === "merge" || action === "merge_push") {
            go({ name: "board" });
          } else {
            flash(action === "commit" ? "Committed." : "Pushed " + ((last && last.branch) || "branch") + ".");
            diffKey = null;
            paint();
          }
        } catch (e) {
          btn.disabled = false;
          btn.textContent = originalLabel;
          flash("error: " + e.message.slice(0, 70));
          updateDiffActions();
        }
      };
      $("#gitprimary").onclick = (e) => run(e.currentTarget.dataset.action, e.currentTarget);
      const menu = $("#gitmenu");
      $("#gitcaret").onclick = (e) => {
        e.stopPropagation();
        menu.hidden = !menu.hidden;
        if (!menu.hidden) {
          const close = (ev) => {
            if (!$(".splitbtn")?.contains(ev.target)) {
              menu.hidden = true;
              document.removeEventListener("mousedown", close);
            }
          };
          setTimeout(() => document.addEventListener("mousedown", close), 0);
        }
      };
      menu.querySelectorAll(".mi").forEach(
        (mi) =>
          (mi.onclick = () => {
            menu.hidden = true;
            run(mi.dataset.action, $("#gitprimary"));
          }),
      );
    } else if (lastDiffState === "building") {
      hint.textContent = "Comment on the diff to request changes — even while the agent is working.";
      actions.innerHTML = "";
    } else {
      hint.textContent = "";
      actions.innerHTML = "";
    }
  }

  const paint = async () => {
    if (App.offline) return; // freeze the view; resume() restarts the flow
    let t;
    try {
      t = await App.call("task.get", { task_id: id });
    } catch {
      return;
    }
    if (!last || last.state !== t.state || last.goal !== t.goal) shell(t);
    last = t;
    const body = $("#tabbody");
    if (tab === "plan") {
      if (t.state === "planning" || t.state === "created") {
        body.innerHTML = '<div class="plan plan-loading">✦ planning agent is drafting the plan…</div>';
        planKey = "drafting";
        planComments.length = 0;
        return;
      }
      let plan = "";
      try {
        plan = (await App.call("task.plan", { task_id: id })).contents;
      } catch {
        /* plan not readable yet */
      }
      const key = t.state + " " + plan;
      // Skip rebuild when nothing changed, so comments / typed text / selection survive the poll.
      if (planKey === key && $("#planbody")) return;
      planKey = key;
      renderPlanTab(t, plan);
    } else {
      let diff = { stat: { files_changed: 0, insertions: 0, deletions: 0 }, files: [], patch: "" };
      try {
        diff = await App.call("task.diff", { task_id: id });
      } catch {
        /* diff not readable yet */
      }
      const files = filterNoiseFiles(parseDiff(diff.patch));
      lastDiffState = t.state;
      const key = t.state + " " + diff.patch;
      const general = $("#dgeneral");
      // Freeze the diff while the user is actively commenting (pending comments,
      // open popover, or text in the general box) so anchors/selection survive.
      const busy = diffComments.length > 0 || hasCommentPop() || (general && (general.value.trim() || document.activeElement === general));
      if ($("#diff-feedback") && (key === diffKey || busy)) {
        updateDiffActions();
        return;
      }
      diffKey = key;
      renderDiffTab(t, files);
    }
  };
  shell(null);
  await paint();
  App.poll = setInterval(paint, 1600);
}
