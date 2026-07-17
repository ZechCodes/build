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
import { mountSplitButton } from "../core/splitButton.js";

// Task states with no more work to run — the run-all control is hidden on them.
const TERMINAL_TASK_STATES = new Set(["merged", "abandoned"]);

// True while a board-level bulk action (Run all / Approve all) is mid-sequence.
// The task poll consults this (stageActionBusy) and skips its rebuild so it never
// remounts a fresh, enabled button under an in-flight multi-RPC loop — which would
// reset the button to look un-pressed and invite a second, conflicting run.
let bulkActionInFlight = false;

/** Whether a stage-board bulk action is mid-flight (task.js freezes its rebuild
 *  while true, the same discipline as an open comment popover). */
export function stageActionBusy() {
  return bulkActionInFlight;
}

/** Pure: which run-all control the stage board shows for a given board.
 *  "stop" when auto-advance is already on; "run" (the split button) when the
 *  task is live and at least one stage is not yet validated; "none" otherwise
 *  (nothing left to run, or a terminal task). Load-bearing in renderStageList. */
export function runAllControlKind({ autoAdvance, stages, taskState }) {
  if (autoAdvance) return "stop";
  if (TERMINAL_TASK_STATES.has(taskState)) return "none";
  // The earliest stage that has not passed gates everything behind it. Run-all
  // only starts it if it is approvable/dispatchable (planned/approved) or already
  // in flight; a validated_failed gate needs a manual fix (the fix bar), and
  // offering "run" there would arm auto-advance yet dispatch nothing.
  const blocking = (stages || []).find((s) => s.state !== "validated_passed");
  if (!blocking || blocking.state === "validated_failed") return "none";
  return "run";
}

// The split-button options for the "run" control (options[0] is the default
// action). Named here so the wiring and any test read the exact copy.
const RUN_ALL_OPTIONS = [
  {
    id: "run_all",
    label: "Run all",
    busyLabel: "Starting…",
    description: "Approve remaining stages and run them all, auto-advancing between each.",
  },
  {
    id: "arm_only",
    menuLabel: "Auto-advance only",
    busyLabel: "Starting…",
    description: "Advance automatically, but approve and start each stage yourself.",
  },
];

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
// comments show the agent's reply and are muted.
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
// a hint on failure, repaint on success.
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

export function renderStagesTab(ctx) {
  const { body, task, stagesData, selectedStageId, callRpc, repaint } = ctx;
  const planId = task.plan_id; // stage docs + comments are plan-scoped
  const runId = task.run_id; // stage execution (dispatch/fix/auto-advance) is run-scoped
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
        await callRpc("plan.comment_delete", { plan_id: planId, comment_id: x.dataset.del });
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

// Pure markup for the stage board (exported for tests). The list container
// carries id="stagelist" — task.js keys its poll freeze/rebuild skip on that
// id, so a rename here silently re-renders the board on every poll tick and
// clobbers in-flight control state (busy buttons, the run-all checkbox).
export function stageBoardHtml(task, stagesData) {
  const stages = stagesData.stages || [];
  const allPlanned = stages.length > 0 && stages.every((s) => s.state === "planned");
  // At the merge gate the final stage's validation report is the decision
  // context: heading + FINDINGS (spec §8.4). notes_for_next_stage is empty on
  // a final stage (there is no next stage), so it can never be the body here.
  const final = stages.length ? stages[stages.length - 1] : null;
  const reviewBanner =
    task.state === "review" && final && final.validation
      ? validationBanner(
          final.validation.passed ? "pass" : "fail",
          `Validation of "${final.title}" ${final.validation.passed ? "passed" : "failed"}`,
          final.validation.findings,
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

  return `
    ${reviewBanner}
    <div class="stagehead">
      <div class="runall" id="runall"></div>
      ${allPlanned ? `<button class="btn mini" id="approveall">Approve all</button>` : ""}
      <span class="hint" id="stageshint"></span>
    </div>
    <div class="stagelist" id="stagelist">${stages.length ? rows : '<div class="empty">No stages yet.</div>'}</div>`;
}

function renderStageList(ctx) {
  const { body, task, stagesData, callRpc, repaint, onSelectStage } = ctx;
  const planId = task.plan_id; // stage docs + comments are plan-scoped
  const runId = task.run_id; // stage execution (dispatch/fix/auto-advance) is run-scoped
  const stages = stagesData.stages || [];
  body.innerHTML = stageBoardHtml(task, stagesData);

  wireRunAllControl(ctx, body.querySelector("#runall"), body.querySelector("#stageshint"));
  const approveAll = body.querySelector("#approveall");
  if (approveAll) {
    bindAction(approveAll, "approving…", body.querySelector("#stageshint"), async () => {
      bulkActionInFlight = true;
      try {
        for (const s of stages.filter((x) => x.state === "planned")) {
          await callRpc("plan.stage_approve", { plan_id: planId, stage_id: s.id });
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

// Mount the run-all control into its host. "run": a split button whose primary
// (run_all) approves every planned stage then turns on auto-advance — the
// bridge's enable-kickstart dispatches stage 1 now that it is Approved — and
// whose menu (arm_only) turns on auto-advance only (the old passive semantics).
// "stop": a plain button that turns auto-advance off. "none": nothing. Errors go
// to the shared hint; the split button restores itself on a rejected run.
function wireRunAllControl(ctx, host, hint) {
  const { task, stagesData, callRpc, repaint } = ctx;
  if (!host) return;
  const planId = task.plan_id; // stage docs + comments are plan-scoped
  const runId = task.run_id; // stage execution (dispatch/fix/auto-advance) is run-scoped
  const stages = stagesData.stages || [];
  const kind = runAllControlKind({ autoAdvance: stagesData.auto_advance, stages, taskState: task.state });

  if (kind === "run") {
    mountSplitButton(host, {
      options: RUN_ALL_OPTIONS,
      run: async (optionId) => {
        bulkActionInFlight = true;
        try {
          if (optionId === "run_all") {
            for (const s of stages.filter((x) => x.state === "planned")) {
              await callRpc("plan.stage_approve", { plan_id: planId, stage_id: s.id });
            }
          }
          await callRpc("run.set_auto_advance", { run_id: runId, enabled: true });
        } catch (e) {
          bulkActionInFlight = false;
          if (hint) hint.textContent = "error: " + e.message.slice(0, 60);
          throw e; // let the split button restore itself for a retry
        }
        bulkActionInFlight = false; // cleared before repaint so the rebuild isn't frozen
        repaint();
      },
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

function renderStageDoc(ctx, stage) {
  const { body, task, stagesData, stageDoc, callRpc, repaint, onSelectStage } = ctx;
  const planId = task.plan_id; // stage docs + comments are plan-scoped
  const runId = task.run_id; // stage execution (dispatch/fix/auto-advance) is run-scoped
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
      await callRpc("run.stage_fix", { run_id: runId, stage_id: stage.id, note });
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
    const addGeneral = body.querySelector("#addgeneral");
    bindAction(addGeneral, "adding…", body.querySelector("#stagehint"), async () => {
      const text = body.querySelector("#stage-general").value.trim();
      if (!text) {
        body.querySelector("#stagehint").textContent = "type a comment first.";
        addGeneral.disabled = false;
        addGeneral.textContent = "Add comment";
        return;
      }
      await callRpc("plan.comment_add", { plan_id: planId, stage_id: stage.id, body: text, anchor: null });
      hideCommentPop();
      repaint();
    });
  }

  renderStageActions(ctx, stage, index, prev);
}

function renderStageActions(ctx, stage, index, prev) {
  const { body, task, stagesData, catalog, callRpc, repaint } = ctx;
  const planId = task.plan_id; // stage docs + comments are plan-scoped
  const runId = task.run_id; // stage execution (dispatch/fix/auto-advance) is run-scoped
  const stages = stagesData.stages || [];
  const actions = body.querySelector("#stageactions");
  const hint = body.querySelector("#stagehint");
  const openCount = stage.open_comments || 0;
  const sendNotesBtn = openCount > 0 ? `<button class="btn" id="sendnotes">Send ${openCount} comment${openCount === 1 ? "" : "s"}</button>` : "";
  const wireSendNotes = () => {
    const b = body.querySelector("#sendnotes");
    if (b) bindAction(b, "sending…", hint, async () => {
      await callRpc("plan.stage_send_notes", { plan_id: planId, stage_id: stage.id });
      repaint();
    });
  };

  if (stage.state === "planned") {
    actions.innerHTML = `${sendNotesBtn}<button class="btn primary" id="approvestage">Approve stage</button>`;
    bindAction(body.querySelector("#approvestage"), "approving…", hint, async () => {
      await callRpc("plan.stage_approve", { plan_id: planId, stage_id: stage.id });
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
        await callRpc("run.stage_dispatch", { run_id: runId, stage_id: stage.id, ...params });
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
