// The run-side Stages tab: a stage *progress* board that joins the run's
// execution progress (Building/Built/Validating/Validated + validation findings)
// with the plan's doc metadata (title, doc sub-state, open-comment count). The
// doc itself and its comment/send-notes flow live on the PLAN route — the doc
// home never moved (single active writer) — so a row click and the open-comment
// badge both link there. This tab owns only the run-scoped execution actions:
// Start stage (run.stage_dispatch) at the sequential gate, Send back to fix
// (run.stage_fix) on a failed stage, and the run-all control (run.set_auto_advance).
// Rendering only; task.js owns the poll loop, the freeze/rebuild key, and the
// plan.stages / run.get fetches (it joins them via joinRunStages before calling in).

import { esc } from "../core/text.js";
import { renderMarkdown } from "../core/markdown.js";
import { slugifyHeading, buildHeadingPath } from "../core/anchors.js";
import { modelOptionsHtml, effortOptionsHtml, effortSupported, modelParams } from "../core/modelPicker.js";

// Run states with no more work to run — the run-all control is hidden on them.
const TERMINAL_RUN_STATES = new Set(["merged", "abandoned", "archived"]);

// True while a board-level bulk action (Run all) is mid-sequence. The task poll
// consults this (stageActionBusy) and skips its rebuild so it never remounts a
// fresh, enabled button under an in-flight multi-RPC loop — which would reset the
// button to look un-pressed and invite a second, conflicting run.
let bulkActionInFlight = false;

/** Whether a stage-board bulk action is mid-flight (task.js freezes its rebuild
 *  while true, the same discipline as an open comment popover). */
export function stageActionBusy() {
  return bulkActionInFlight;
}

/** Join the plan's stage docs (title, summary, doc sub-state, open comments)
 *  with the run's per-stage execution progress (progress state, validation,
 *  start_sha) by stage id. A stage the run has not dispatched yet has no progress
 *  record, so its effective `state` is the plan doc's sub-state (planned/approved);
 *  once dispatched, the run's progress state (building…validated_*) takes over.
 *  Pure — the single source of truth the whole run-side board renders from. */
export function joinRunStages(planStages, runStages) {
  const progressById = new Map((runStages || []).map((p) => [p.id, p]));
  return (planStages || []).map((doc) => {
    const progress = progressById.get(doc.id) || null;
    return {
      id: doc.id,
      title: doc.title,
      summary: doc.summary,
      doc_state: doc.state, // planned | approved (the plan's gate)
      open_comments: doc.open_comments || 0,
      state: progress ? progress.state : doc.state, // effective: progress wins
      validation: progress ? progress.validation : null,
      start_sha: progress ? progress.start_sha : null,
    };
  });
}

/** Pure: which run-all control the stage board shows for a given board.
 *  "stop" when auto-advance is already on; "run" (the split button) when the run
 *  is live and the earliest unfinished stage is runnable; "none" otherwise
 *  (nothing left to run, a terminal run, or the gate is a validated_failed stage
 *  that needs a manual fix). Load-bearing in wireRunAllControl. */
export function runAllControlKind({ autoAdvance, stages, taskState }) {
  if (autoAdvance) return "stop";
  if (TERMINAL_RUN_STATES.has(taskState)) return "none";
  // The earliest stage that has not passed gates everything behind it. Run-all
  // only starts it if it is approvable/dispatchable (planned/approved) or already
  // in flight; a validated_failed gate needs a manual fix (the fix bar), and
  // offering "run" there would arm auto-advance yet dispatch nothing.
  const blocking = (stages || []).find((s) => s.state !== "validated_passed");
  if (!blocking || blocking.state === "validated_failed") return "none";
  return "run";
}

/** Why the earliest unfinished (gate) stage cannot be Started yet, as human copy
 *  for a disabled action — or null when Start is ready. Ordered by the bridge's
 *  own `dispatch_run_stage` gate: the doc must be Approved, every earlier stage
 *  must have passed validation on this run, and the run must be parked at the
 *  stage gate (a stage already building blocks a manual dispatch). */
export function stageGateReason(stages, index, runState) {
  const stage = stages[index];
  if (!stage) return "No stage.";
  if (stage.doc_state !== "approved") return `Approve “${stage.title}” on the plan before starting it.`;
  const blocker = stages.slice(0, index).find((s) => s.state !== "validated_passed");
  if (blocker) return `Waiting on validation of “${blocker.title}”.`;
  if (runState !== "stage_gate") return "A stage is already running.";
  return null;
}

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

const commentBadge = (n) => (n > 0 ? `<span class="cbadge">${n} 💬</span>` : "");

// The enclosing heading chain for a selection anchor inside a rendered doc:
// collect the h1/h2/h3 at or before the anchor node, then reduce to the
// enclosing chain (anchors.js). Reused by planStages.js's comment composer.
export function headingPathFor(docEl, anchorNode) {
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
// comments show the agent's reply and are muted. Reused by planStages.js.
export function commentCard(comment) {
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
// a hint on failure, repaint on success. Reused by planStages.js.
export function bindAction(button, busyLabel, hintEl, run) {
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

// Pure markup for the run-side stage progress board (exported for tests). Rows
// carry the effective-state chip, the plan's open-comment badge, and a failed
// stage's findings inline. The list container keeps id="stagelist" — task.js
// keys its poll freeze/rebuild skip on that id, so a rename here silently
// re-renders the board every tick and clobbers in-flight control state.
export function stageBoardHtml(run, stagesData) {
  const stages = stagesData.stages || [];
  // At the merge gate the final stage's validation report is the decision
  // context: heading + FINDINGS (spec §8.4). notes_for_next_stage is empty on
  // a final stage (there is no next stage), so it can never be the body here.
  const final = stages.length ? stages[stages.length - 1] : null;
  const reviewBanner =
    run.state === "review" && final && final.validation
      ? validationBanner(
          final.validation.passed ? "pass" : "fail",
          `Validation of "${final.title}" ${final.validation.passed ? "passed" : "failed"}`,
          final.validation.findings,
        )
      : "";

  const rows = stages
    .map((s, i) => {
      const num = String(i + 1).padStart(2, "0");
      const failFindings =
        s.state === "validated_failed" && s.validation
          ? validationBanner("fail", "Validation failed", s.validation.findings)
          : "";
      return `<div class="stagerow" data-stage="${esc(s.id)}">
        <span class="stagenum">${num}</span>
        <div class="stagemain">
          <div class="stagetop"><span class="stagetitle">${esc(s.title)}</span>
            <span class="chip stagechip ${stageChipClass(s.state)}">${STAGE_LABEL[s.state] || s.state}</span>
            ${commentBadge(s.open_comments)}</div>
          ${s.summary ? `<div class="stagesummary">${esc(s.summary)}</div>` : ""}
          ${failFindings}
        </div></div>`;
    })
    .join("");

  return `
    ${reviewBanner}
    <div class="stagehead">
      <div class="runall" id="runall"></div>
      <span class="hint" id="stageshint"></span>
    </div>
    <div class="stagelist" id="stagelist">${stages.length ? rows : '<div class="empty">No stages yet.</div>'}</div>
    <div class="stageaction" id="stageaction"></div>`;
}

/** Render the run-side stage board and wire its run-scoped actions. ctx:
 *  { body, run, stagesData:{stages(joined), auto_advance}, catalog, callRpc,
 *    repaint, openPlan(stageId) }. */
export function renderStagesTab(ctx) {
  const { body, run, stagesData, callRpc, repaint, openPlan } = ctx;
  const stages = stagesData.stages || [];
  body.innerHTML = stageBoardHtml(run, stagesData);

  wireRunAllControl(ctx, body.querySelector("#runall"), body.querySelector("#stageshint"));

  // A row and its comment badge both open the plan's stage doc — the doc home
  // never moved to the run (single active writer).
  body.querySelectorAll(".stagerow").forEach((row) => {
    row.onclick = () => openPlan(row.dataset.stage);
  });

  renderGateAction(ctx, body.querySelector("#stageaction"));
}

// The single actionable stage's controls, below the board. The earliest
// unfinished stage gates everything behind it, so at most one is actionable:
// a validated_failed stage → the fix bar; an approved+ready stage → Start; an
// approved-but-blocked stage → a disabled Start with the gate reason.
function renderGateAction(ctx, host) {
  const { run, stagesData, catalog, callRpc, repaint, openPlan } = ctx;
  if (!host) return;
  const stages = stagesData.stages || [];
  const index = stages.findIndex((s) => s.state !== "validated_passed");
  if (index < 0) return; // every stage validated — the merge gate lives on Changes
  const stage = stages[index];
  const runId = run.run_id;

  if (stage.state === "validated_failed") {
    host.innerHTML = `
      <div class="fixbar"><div class="fixlabel">Fix “${esc(stage.title)}” and re-validate</div>
        <textarea id="fixnote" class="plan-general" placeholder="Optional note for the fix agent…"></textarea>
        <div class="actionbar"><span class="hint" id="stagehint"></span>
          <div class="right"><button class="btn" id="fixplan">Comment on the plan →</button>
            <button class="btn primary" id="sendfix">Send back to fix</button></div></div></div>`;
    host.querySelector("#fixplan").onclick = () => openPlan(stage.id);
    bindAction(host.querySelector("#sendfix"), "sending…", host.querySelector("#stagehint"), async () => {
      const note = host.querySelector("#fixnote").value.trim();
      await callRpc("run.stage_fix", { run_id: runId, stage_id: stage.id, note });
      repaint();
    });
    return;
  }

  if (stage.state === "building" || stage.state === "built") {
    host.innerHTML = `<div class="actionbar"><span class="hint">Agent building “${esc(stage.title)}”…</span></div>`;
    return;
  }
  if (stage.state === "validating") {
    host.innerHTML = `<div class="actionbar"><span class="hint">Validating “${esc(stage.title)}”…</span></div>`;
    return;
  }

  // planned / approved: the Start control, enabled only at the sequential gate.
  const reason = stageGateReason(stages, index, run.state);
  const models = (catalog && catalog.models) || [];
  const efforts = (catalog && catalog.efforts) || [];
  if (reason) {
    // Not startable yet. A planned doc points the user at the plan to approve it;
    // otherwise the reason is informational (a stage is running).
    const toPlan = stage.doc_state !== "approved"
      ? `<button class="btn" id="gotoplan">Open the plan →</button>`
      : "";
    host.innerHTML = `<div class="actionbar"><span class="hint">${esc(reason)}</span>
      <div class="right">${toPlan}<button class="btn primary" disabled>Start “${esc(stage.title)}”</button></div></div>`;
    const g = host.querySelector("#gotoplan");
    if (g) g.onclick = () => openPlan(stage.id);
    return;
  }
  host.innerHTML = `<div class="actionbar"><span class="hint" id="stagehint"></span>
    <div class="right">
      <select id="stModel" class="mini" title="Coding agent model">${modelOptionsHtml(models, run.model)}</select>
      <select id="stEffort" class="mini" title="Reasoning effort">${effortOptionsHtml(efforts, run.effort)}</select>
      <button class="btn primary" id="startstage">Start “${esc(stage.title)}”</button></div></div>`;
  const modelSel = host.querySelector("#stModel");
  const effortSel = host.querySelector("#stEffort");
  const syncEffort = () => {
    const supported = effortSupported(models, modelSel.value);
    effortSel.disabled = !supported;
    if (!supported) effortSel.value = "";
  };
  modelSel.onchange = syncEffort;
  syncEffort();
  bindAction(host.querySelector("#startstage"), "starting…", host.querySelector("#stagehint"), async () => {
    const params = modelParams(models, modelSel.value, effortSel.value);
    await callRpc("run.stage_dispatch", { run_id: runId, stage_id: stage.id, ...params });
    repaint();
  });
}

// Mount the run-all control into its host. "run": a plain button that turns on
// auto-advance — the bridge's enable-kickstart dispatches the next approved
// stage and chains each one to its verdict (stage approval itself is a plan-side
// gate, so run-all only runs stages already approved). "stop": a plain button
// that turns auto-advance off. "none": nothing.
function wireRunAllControl(ctx, host, hint) {
  const { run, stagesData, callRpc, repaint } = ctx;
  if (!host) return;
  const runId = run.run_id;
  const stages = stagesData.stages || [];
  const kind = runAllControlKind({ autoAdvance: stagesData.auto_advance, stages, taskState: run.state });

  if (kind === "run") {
    host.innerHTML = `<button class="btn mini" id="runallbtn">Run all</button>`;
    bindAction(host.querySelector("#runallbtn"), "starting…", hint, async () => {
      bulkActionInFlight = true;
      try {
        await callRpc("run.set_auto_advance", { run_id: runId, enabled: true });
      } finally {
        bulkActionInFlight = false;
      }
      repaint();
    });
    return;
  }
  if (kind === "stop") {
    host.innerHTML = `<button class="btn mini" id="stopadvance">Stop auto-advance</button>`;
    bindAction(host.querySelector("#stopadvance"), "stopping…", hint, async () => {
      await callRpc("run.set_auto_advance", { run_id: runId, enabled: false });
      repaint();
    });
  }
}
