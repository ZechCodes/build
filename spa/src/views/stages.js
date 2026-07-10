// The multi-stage plan tab: a stage board (per-stage state, open-comment counts)
// and a per-stage doc view with persisted, server-side comments, the previous
// stage's validation banner, and per-stage actions (approve / send notes / start
// / fix). Rendering only — task.js owns the poll loop, freeze/rebuild key, and
// the task.get / task.stages / task.stage_doc RPC fetches; every mutation here
// goes through ctx.callRpc + ctx.repaint so it persists and re-polls.

import { esc } from "../core/text.js";
import { renderMarkdown } from "../core/markdown.js";
import { slugifyHeading, buildHeadingPath } from "../core/anchors.js";
import { modelOptionsHtml, effortOptionsHtml, effortSupported, modelParams } from "../core/modelPicker.js";
import { watchSelection } from "../selectWatch.js";
import { showCommentPop, hideCommentPop } from "../commentPop.js";

export const STAGE_LABEL = {
  planned: "PLANNED",
  approved: "APPROVED",
  building: "BUILDING",
  built: "BUILT",
  validating: "VALIDATING",
  validated_passed: "VALIDATED",
  validated_failed: "VALIDATION FAILED",
};

// Reuse the chip palette (see shared.js chipClass / styles.css .chip.*): planned
// is neutral (no class), approved awaits the human (attn), in-flight sub-states
// are work, a passed stage is done, a failed one warns.
export function stageChipClass(state) {
  if (state === "approved") return "attn";
  if (state === "building" || state === "built" || state === "validating") return "work";
  if (state === "validated_passed") return "done";
  if (state === "validated_failed") return "warn";
  return "";
}

// Document-level selection watcher for the stage doc; disposed on each render so
// the 1.6s poll never accumulates listeners (same discipline as task.js).
let stageSelDispose = null;

const commentBadge = (n) => (n > 0 ? `<span class="cbadge">${n} 💬</span>` : "");

// The enclosing heading chain for a selection anchor inside the rendered stage
// doc: collect the h1/h2/h3 positioned at or before the anchor node, then reduce
// to the enclosing chain (anchors.js). View-side; not unit-tested.
function headingPathFor(docEl, anchorNode) {
  const preceding = Array.from(docEl.querySelectorAll("h1, h2, h3"))
    .filter((h) => h.compareDocumentPosition(anchorNode) & Node.DOCUMENT_POSITION_FOLLOWING || h.contains(anchorNode))
    .map((h) => ({ level: +h.tagName.slice(1), text: h.textContent }));
  return buildHeadingPath(preceding);
}

function validationBanner(kind, heading, bodyMarkdown) {
  const body = bodyMarkdown && bodyMarkdown.trim() ? `<div class="sv-body">${renderMarkdown(bodyMarkdown)}</div>` : "";
  return `<div class="stage-validation ${kind}"><div class="sv-head">${esc(heading)}</div>${body}</div>`;
}

// One persisted comment card. Open comments carry a delete affordance; addressed
// comments show the agent's reply and are muted.
function commentCard(comment) {
  const anchor = comment.anchor;
  const breadcrumb = anchor
    ? anchor.heading_path && anchor.heading_path.length
      ? esc(anchor.heading_path.join(" > "))
      : "(top of doc)"
    : "(general)";
  const lastHeading = anchor && anchor.heading_path && anchor.heading_path.length ? anchor.heading_path[anchor.heading_path.length - 1] : "";
  const snippet = anchor && anchor.snippet ? `<span class="cc-snip">${esc(anchor.snippet.replace(/\s+/g, " ").trim().slice(0, 200))}</span>` : "";
  const addressed = comment.state === "addressed";
  const reply = addressed && comment.agent_reply ? `<div class="cc-reply"><span class="cc-reply-k">agent</span> ${esc(comment.agent_reply)}</div>` : "";
  const del = addressed ? "" : `<span class="cc-x" data-del="${esc(comment.id)}">×</span>`;
  return `<div class="commentcard${addressed ? " addressed" : ""}" data-id="${esc(comment.id)}">
    ${del}
    <span class="cc-crumb" data-scroll="${esc(slugifyHeading(lastHeading))}">${breadcrumb}</span>
    ${snippet}
    <span class="cc-body">${esc(comment.body)}</span>
    ${reply}</div>`;
}

// Bind an async RPC to a button: disable + label while in flight, restore + show
// a hint on failure, repaint on success.
function bindAction(button, busyLabel, hintEl, run) {
  button.onclick = async () => {
    const original = button.textContent;
    button.disabled = true;
    button.textContent = busyLabel;
    try {
      await run();
    } catch (e) {
      button.disabled = false;
      button.textContent = original;
      if (hintEl) hintEl.textContent = "error: " + e.message.slice(0, 60);
    }
  };
}

export function renderStagesTab(ctx) {
  const { body, task, stagesData, selectedStageId, callRpc, repaint } = ctx;
  const taskId = task.task_id;
  const stages = stagesData.stages || [];
  if (stageSelDispose) {
    stageSelDispose();
    stageSelDispose = null;
  }
  const selected = selectedStageId ? stages.find((s) => s.id === selectedStageId) : null;
  if (selected) renderStageDoc(ctx, selected);
  else renderStageList(ctx);

  // Shared: wire the delete/scroll affordances present in either view.
  body.querySelectorAll(".cc-x[data-del]").forEach((x) => {
    x.onclick = async (e) => {
      e.stopPropagation();
      try {
        await callRpc("task.comment_delete", { task_id: taskId, comment_id: x.dataset.del });
        repaint();
      } catch {
        /* the poll will re-sync */
      }
    };
  });
  body.querySelectorAll(".cc-crumb[data-scroll]").forEach((c) => {
    c.onclick = () => {
      const id = c.dataset.scroll;
      if (!id) return;
      const target = body.querySelector("#stagedoc #" + CSS.escape(id));
      if (target) target.scrollIntoView({ behavior: "smooth", block: "center" });
    };
  });
}

function renderStageList(ctx) {
  const { body, task, stagesData, callRpc, repaint, onSelectStage } = ctx;
  const taskId = task.task_id;
  const stages = stagesData.stages || [];
  const allPlanned = stages.length > 0 && stages.every((s) => s.state === "planned");
  // At the merge gate the final stage's validation report is the decision context.
  const reviewBanner =
    task.state === "review" && stages.length && stages[stages.length - 1].validation
      ? validationBanner(
          stages[stages.length - 1].validation.passed ? "pass" : "fail",
          `Validation of "${stages[stages.length - 1].title}" ${stages[stages.length - 1].validation.passed ? "passed" : "failed"}`,
          stages[stages.length - 1].validation.passed ? stages[stages.length - 1].validation.notes_for_next_stage : stages[stages.length - 1].validation.findings,
        )
      : "";

  const rows = stages
    .map((s, i) => {
      const num = String(i + 1).padStart(2, "0");
      return `<div class="stagerow" data-stage="${esc(s.id)}">
        <span class="stagenum">${num}</span>
        <div class="stagemain">
          <div class="stagetop"><span class="stagetitle">${esc(s.title)}</span>
            <span class="chip stagechip ${stageChipClass(s.state)}">${STAGE_LABEL[s.state] || s.state}</span>
            ${commentBadge(s.open_comments)}</div>
          ${s.summary ? `<div class="stagesummary">${esc(s.summary)}</div>` : ""}
        </div></div>`;
    })
    .join("");

  body.innerHTML = `
    ${reviewBanner}
    <div class="stagehead">
      <label class="runall"><input type="checkbox" id="runall"${stagesData.auto_advance ? " checked" : ""}/> Run all (auto-advance)</label>
      ${allPlanned ? `<button class="btn mini" id="approveall">Approve all</button>` : ""}
      <span class="hint" id="stageshint"></span>
    </div>
    <div class="stagelist">${stages.length ? rows : '<div class="empty">No stages yet.</div>'}</div>`;

  const runall = body.querySelector("#runall");
  runall.onchange = async () => {
    runall.disabled = true;
    try {
      await callRpc("task.set_auto_advance", { task_id: taskId, enabled: runall.checked });
      repaint();
    } catch (e) {
      runall.disabled = false;
      runall.checked = !runall.checked;
      body.querySelector("#stageshint").textContent = "error: " + e.message.slice(0, 60);
    }
  };
  const approveAll = body.querySelector("#approveall");
  if (approveAll) {
    bindAction(approveAll, "approving…", body.querySelector("#stageshint"), async () => {
      for (const s of stages.filter((x) => x.state === "planned")) {
        await callRpc("task.stage_approve", { task_id: taskId, stage_id: s.id });
      }
      repaint();
    });
  }
  body.querySelectorAll(".stagerow").forEach((row) => {
    row.onclick = () => onSelectStage(row.dataset.stage);
  });
}

function renderStageDoc(ctx, stage) {
  const { body, task, stagesData, stageDoc, callRpc, repaint, onSelectStage } = ctx;
  const taskId = task.task_id;
  const stages = stagesData.stages || [];
  const index = stages.findIndex((s) => s.id === stage.id);
  const prev = index > 0 ? stages[index - 1] : null;

  // The previous stage's validation report is surfaced here (on stage N+1).
  let banner = "";
  if (prev && prev.validation) {
    banner = prev.validation.passed
      ? validationBanner("pass", `Validation of "${prev.title}" passed`, prev.validation.notes_for_next_stage)
      : validationBanner("fail", `Validation of "${prev.title}" failed`, prev.validation.findings);
  }
  // This stage failed its own validation: its findings + a send-back-to-fix bar.
  const ownFail = stage.state === "validated_failed" && stage.validation;
  const ownFailBanner = ownFail ? validationBanner("fail", `This stage's validation failed`, stage.validation.findings) : "";

  const comments = (stage.comments || []).slice().sort((a, b) => (a.state === b.state ? 0 : a.state === "open" ? -1 : 1));
  const commentsHtml = comments.length ? comments.map(commentCard).join("") : "";
  const canComment = stage.state === "planned" || stage.state === "approved";

  const docContents = stageDoc && stageDoc.stage_id === stage.id ? stageDoc.contents : "";

  body.innerHTML = `
    <div class="stageback" id="stageback">← All stages</div>
    ${banner}
    ${ownFailBanner}
    ${ownFail ? `<div class="fixbar"><textarea id="fixnote" class="plan-general" placeholder="Optional note for the fix agent…"></textarea>
      <div class="actionbar"><span class="hint" id="fixhint"></span><div class="right"><button class="btn primary" id="sendfix">Send back to fix</button></div></div></div>` : ""}
    <div class="plan" id="stagedoc">${docContents ? renderMarkdown(docContents) : '<div class="plan-loading">✦ loading stage document…</div>'}</div>
    <div class="stagecomments">${commentsHtml}</div>
    ${canComment ? `<div class="plan-feedback"><textarea id="stage-general" class="plan-general" placeholder="Add a general comment on this stage…"></textarea>
      <div class="right"><button class="btn mini" id="addgeneral">Add comment</button></div></div>` : ""}
    <div class="actionbar"><span class="hint" id="stagehint"></span><div class="right" id="stageactions"></div></div>`;

  body.querySelector("#stageback").onclick = () => onSelectStage(null);

  if (ownFail) {
    bindAction(body.querySelector("#sendfix"), "sending…", body.querySelector("#fixhint"), async () => {
      const note = body.querySelector("#fixnote").value.trim();
      await callRpc("task.stage_fix", { task_id: taskId, stage_id: stage.id, note });
      repaint();
    });
  }

  // Anchored comments: select text in the doc → popover → task.comment_add.
  if (canComment) {
    const docEl = body.querySelector("#stagedoc");
    stageSelDispose = watchSelection(docEl, (sel) => {
      const anchorNode = sel.anchorNode;
      const snippet = sel.toString().trim().slice(0, 400);
      const range = sel.getRangeAt(0);
      showCommentPop(range.getBoundingClientRect(), async (commentBody) => {
        try {
          await callRpc("task.comment_add", {
            task_id: taskId,
            stage_id: stage.id,
            body: commentBody,
            anchor: { heading_path: headingPathFor(docEl, anchorNode), snippet },
          });
          window.getSelection().removeAllRanges();
          repaint();
        } catch {
          /* poll re-syncs */
        }
      });
    });
    const addGeneral = body.querySelector("#addgeneral");
    bindAction(addGeneral, "adding…", body.querySelector("#stagehint"), async () => {
      const text = body.querySelector("#stage-general").value.trim();
      if (!text) {
        body.querySelector("#stagehint").textContent = "type a comment first.";
        addGeneral.disabled = false;
        addGeneral.textContent = "Add comment";
        return;
      }
      await callRpc("task.comment_add", { task_id: taskId, stage_id: stage.id, body: text, anchor: null });
      hideCommentPop();
      repaint();
    });
  }

  renderStageActions(ctx, stage, index, prev);
}

function renderStageActions(ctx, stage, index, prev) {
  const { body, task, stagesData, catalog, callRpc, repaint } = ctx;
  const taskId = task.task_id;
  const stages = stagesData.stages || [];
  const actions = body.querySelector("#stageactions");
  const hint = body.querySelector("#stagehint");
  const openCount = stage.open_comments || 0;
  const sendNotesBtn = openCount > 0 ? `<button class="btn" id="sendnotes">Send ${openCount} comment${openCount === 1 ? "" : "s"}</button>` : "";
  const wireSendNotes = () => {
    const b = body.querySelector("#sendnotes");
    if (b) bindAction(b, "sending…", hint, async () => {
      await callRpc("task.stage_send_notes", { task_id: taskId, stage_id: stage.id });
      repaint();
    });
  };

  if (stage.state === "planned") {
    actions.innerHTML = `${sendNotesBtn}<button class="btn primary" id="approvestage">Approve stage</button>`;
    bindAction(body.querySelector("#approvestage"), "approving…", hint, async () => {
      await callRpc("task.stage_approve", { task_id: taskId, stage_id: stage.id });
      repaint();
    });
    wireSendNotes();
    return;
  }
  if (stage.state === "approved") {
    const priorsPassed = stages.slice(0, index).every((s) => s.state === "validated_passed");
    const ready = priorsPassed && task.state === "plan_review";
    const models = (catalog && catalog.models) || [];
    const efforts = (catalog && catalog.efforts) || [];
    actions.innerHTML = `${sendNotesBtn}
      <select id="stModel" class="mini" title="Coding agent model">${modelOptionsHtml(models, task.model)}</select>
      <select id="stEffort" class="mini" title="Reasoning effort">${effortOptionsHtml(efforts, task.effort)}</select>
      <button class="btn primary" id="startstage"${ready ? "" : " disabled"}>Start stage</button>`;
    if (!ready) {
      const blocker = stages.slice(0, index).find((s) => s.state !== "validated_passed");
      hint.textContent = blocker ? `waiting on validation of "${blocker.title}"` : "waiting on the plan review gate";
    }
    const modelSel = body.querySelector("#stModel");
    const effortSel = body.querySelector("#stEffort");
    const syncEffort = () => {
      const supported = effortSupported(models, modelSel.value);
      effortSel.disabled = !supported;
      if (!supported) effortSel.value = "";
    };
    modelSel.onchange = syncEffort;
    syncEffort();
    if (ready) {
      bindAction(body.querySelector("#startstage"), "starting…", hint, async () => {
        const params = modelParams(models, modelSel.value, effortSel.value);
        await callRpc("task.stage_dispatch", { task_id: taskId, stage_id: stage.id, ...params });
        repaint();
      });
    }
    wireSendNotes();
    return;
  }
  if (stage.state === "building" || stage.state === "built") {
    actions.innerHTML = "";
    hint.textContent = "agent working on this stage…";
    return;
  }
  if (stage.state === "validating") {
    actions.innerHTML = "";
    hint.textContent = "validation running…";
    return;
  }
  if (stage.state === "validated_passed") {
    actions.innerHTML = "";
    hint.textContent = "stage complete";
    return;
  }
  // validated_failed → the fix bar above owns the action.
  actions.innerHTML = "";
}
