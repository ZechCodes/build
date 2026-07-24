// The plan-side half of the multi-stage surface: a stage board of docs and a
// per-stage doc view with per-stage approve (plan.stage_approve), anchored
// persisted comments (plan.comment_add / plan.comment_delete), and send-notes
// per stage (plan.stage_send_notes). This mirrors views/stages.js but carries
// only the plan-scoped actions — stage *execution* (dispatch/fix/auto-advance,
// the Building/Validating sub-states) lives on the run, so nothing here touches
// run.*. The low-level comment machinery (commentCard, headingPathFor,
// bindAction) is reused from stages.js so the two boards stay in visual and
// behavioural lockstep. Rendering only; plan.js owns the poll loop, the
// freeze/rebuild key, and the plan.stages / plan.stage_doc fetches.

import { esc } from "../core/text.js";
import { renderMarkdown } from "../core/markdown.js";
import { STAGE_LABEL, stageChipClass, commentCard, headingPathFor, bindAction } from "./stages.js";

// The honest empty-state copy for a doc pane whose canonical contents are gone
// (a migrated plan predating canonical storage, its worktree pruned). Shared by
// the single-doc pane (plan.js) so both surfaces say the same thing.
export const DOCS_UNAVAILABLE =
  "This plan's documents are unavailable — they predate canonical doc storage and their worktree is gone.";
import { stageNotesTarget } from "../core/taskActions.js";
import { watchSelection } from "../selectWatch.js";
import { showCommentPop } from "../commentPop.js";

// True while an Approve-all sweep is mid-flight. plan.js consults this
// (planStageActionBusy) and skips its poll rebuild so the in-flight button is
// never remounted enabled under the running loop — the same discipline the run
// stage board uses for its bulk controls.
let bulkActionInFlight = false;

/** Whether an Approve-all sweep is mid-flight (plan.js freezes its rebuild
 *  while true, matching the open-comment-popover discipline). */
export function planStageActionBusy() {
  return bulkActionInFlight;
}

// Document-level selection watcher for the stage doc, disposed on each render so
// the poll never accumulates listeners (same discipline as stages.js/task.js).
let stageSelDispose = null;

const commentBadge = (n) => (n > 0 ? `<span class="cbadge">${n} 💬</span>` : "");

/** The doc-read error pane, with an inline Retry that clears the read latch and
 *  refetches (W15) — replacing the old "Reopen the plan/stage to retry" copy.
 *  `kind` is "plan" (the single-doc pane in plan.js) or "stage" (this file's
 *  stage-doc pane); each carries the button id its host wires. Pure/exported so
 *  both surfaces render the same affordance and it can be tested directly. */
export function docErrorPaneHtml(kind) {
  const isStage = kind === "stage";
  const buttonId = isStage ? "stagedocretry" : "docretry";
  const what = isStage ? "this stage document" : "the plan document";
  return `<div class="plan-empty warn">Couldn't load ${what}. <button class="btn mini" id="${buttonId}">Retry</button></div>`;
}

/** Pure markup for the plan-side stage board (exported for tests). Manifest
 *  rows carry the doc sub-state chip (planned/approved), the stage summary, and
 *  the open-comment badge; an Approve-all control shows only while every stage
 *  is still planned. The list container keeps id="stagelist" so plan.js's poll
 *  freeze/rebuild skip can find it — a rename here would silently re-render the
 *  board every tick and clobber in-flight control state. */
export function planStageBoardHtml(plan, stagesData) {
  const stages = stagesData.stages || [];
  const allPlanned = stages.length > 0 && stages.every((s) => s.state === "planned");
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
  return `
    <div class="stagehead">
      ${allPlanned ? `<button class="btn mini" id="approveall">Approve all stages</button>` : ""}
      <span class="hint" id="stageshint"></span>
    </div>
    <div class="stagelist" id="stagelist">${stages.length ? rows : '<div class="empty">No stages yet.</div>'}</div>`;
}

export function renderPlanStages(ctx) {
  const { body, selectedStageId } = ctx;
  const stages = ctx.stagesData.stages || [];
  if (stageSelDispose) {
    stageSelDispose();
    stageSelDispose = null;
  }
  const selected = selectedStageId ? stages.find((s) => s.id === selectedStageId) : null;
  if (selected) renderStageDoc(ctx, selected);
  else renderStageList(ctx);

  // Shared: the delete/scroll affordances live in either view.
  body.querySelectorAll(".cc-x[data-del]").forEach((x) => {
    x.onclick = async (e) => {
      e.stopPropagation();
      try {
        await ctx.callRpc("plan.comment_delete", { plan_id: ctx.plan.plan_id, comment_id: x.dataset.del });
        ctx.repaint();
      } catch {
        /* the poll re-syncs */
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
  const { body, plan, stagesData, callRpc, repaint, onSelectStage } = ctx;
  const stages = stagesData.stages || [];
  body.innerHTML = planStageBoardHtml(plan, stagesData);

  const approveAll = body.querySelector("#approveall");
  if (approveAll) {
    bindAction(approveAll, "approving…", async () => {
      bulkActionInFlight = true;
      try {
        for (const s of stages.filter((x) => x.state === "planned")) {
          await callRpc("plan.stage_approve", { plan_id: plan.plan_id, stage_id: s.id });
        }
      } finally {
        bulkActionInFlight = false;
      }
      repaint();
    });
  }
  body.querySelectorAll(".stagerow").forEach((row) => {
    row.onclick = () => onSelectStage(row.dataset.stage);
  });
}

function renderStageDoc(ctx, stage) {
  const { body, plan, stageDoc, callRpc, repaint, onSelectStage } = ctx;
  const planId = plan.plan_id;

  const comments = (stage.comments || []).slice().sort((a, b) => (a.state === b.state ? 0 : a.state === "open" ? -1 : 1));
  const commentsHtml = comments.length ? comments.map(commentCard).join("") : "";
  // Comments are accepted on planned/approved docs only (the bridge enforces
  // the same); a terminal/other doc state is read-only here.
  const canComment = stage.state === "planned" || stage.state === "approved";
  const docContents = stageDoc && stageDoc.stage_id === stage.id ? stageDoc.contents : "";
  // plan.js decides the pane state (unavailable / error / ready / loading) since
  // it owns the fetch, the docs_available flag, and the per-stage error latch.
  // A doc that is not readable never mounts the comment machinery.
  const paneState = ctx.stageDocState || (docContents ? "ready" : "loading");
  const docHtml =
    paneState === "ready" ? renderMarkdown(docContents)
    : paneState === "unavailable" ? `<div class="plan-empty">${esc(DOCS_UNAVAILABLE)}</div>`
    : paneState === "error" ? docErrorPaneHtml("stage")
    : '<div class="plan-loading">✦ loading stage document…</div>';
  const canAnnotate = canComment && paneState === "ready";

  body.innerHTML = `
    <div class="stageback" id="stageback">← All stages</div>
    <div class="plan" id="stagedoc">${docHtml}</div>
    <div class="stagecomments">${commentsHtml}</div>
    <div class="actionbar"><span class="hint" id="stagehint"></span><div class="right" id="stageactions"></div></div>`;

  body.querySelector("#stageback").onclick = () => onSelectStage(null);

  // A stage doc-read error latched the pane; the inline Retry clears the latch
  // and refetches through plan.js's onRetryStageDoc callback (W15).
  const stageRetry = body.querySelector("#stagedocretry");
  if (stageRetry && ctx.onRetryStageDoc) stageRetry.onclick = () => ctx.onRetryStageDoc(stage.id);

  // Anchored comments: select text in the doc → popover → plan.comment_add.
  if (canAnnotate) {
    const docEl = body.querySelector("#stagedoc");
    stageSelDispose = watchSelection(docEl, (sel) => {
      const anchorNode = sel.anchorNode;
      const snippet = sel.toString().trim().slice(0, 400);
      const range = sel.getRangeAt(0);
      showCommentPop(range.getBoundingClientRect(), async (commentBody) => {
        try {
          await callRpc("plan.comment_add", {
            plan_id: planId,
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
  }

  renderStageActions(ctx, stage);
}

// Plan-side per-stage actions: send the stage's open comments back for a
// revision, and — while the doc is still planned — approve it (plan.stage_approve).
// The revision verb depends on the plan's lifecycle (stageNotesTarget): under
// review it is plan.stage_send_notes; once the plan is approved with a live run
// the bridge rejects that, so the notes route through the run's mid-run session
// (run.stage_send_notes). An approved plan with no run has no session to revise
// through — the send-notes button is disabled.
function renderStageActions(ctx, stage) {
  const { body, plan, callRpc, repaint } = ctx;
  const planId = plan.plan_id;
  const actions = body.querySelector("#stageactions");
  const hint = body.querySelector("#stagehint");
  const openCount = stage.open_comments || 0;
  const target = stageNotesTarget(plan);
  const viaRun = target && target.method === "run.stage_send_notes";
  const sendNotesBtn = openCount > 0 ? `<button class="btn" id="sendnotes">Send ${openCount} comment${openCount === 1 ? "" : "s"}</button>` : "";
  const wireSendNotes = () => {
    const b = body.querySelector("#sendnotes");
    if (!b) return;
    if (!target) {
      // Plan approved but no run to revise through: nothing to send to.
      b.disabled = true;
      b.title = "Approve settled — start a run to revise this stage.";
      return;
    }
    b.title = viaRun ? "Revises this stage through the run's stage-gate revision." : "";
    bindAction(b, "sending…", async () => {
      const params = viaRun ? { run_id: target.entityId, stage_id: stage.id } : { plan_id: planId, stage_id: stage.id };
      await callRpc(target.method, params);
      repaint();
    });
  };

  if (stage.state === "planned") {
    actions.innerHTML = `${sendNotesBtn}<button class="btn primary" id="approvestage">Approve stage</button>`;
    bindAction(body.querySelector("#approvestage"), "approving…", async () => {
      await callRpc("plan.stage_approve", { plan_id: planId, stage_id: stage.id });
      repaint();
    });
    wireSendNotes();
    return;
  }
  // approved (or any non-planned doc state): send-notes stays available; the
  // doc is otherwise settled on the plan side.
  actions.innerHTML = sendNotesBtn;
  hint.textContent = viaRun ? "Sends to the run's stage-gate revision." : "";
  wireSendNotes();
  if (!sendNotesBtn && !hint.textContent) body.querySelector(".actionbar")?.remove();
}
