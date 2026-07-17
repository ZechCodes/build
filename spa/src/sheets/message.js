// Freeform message to a run's agent: redirects a live session or resumes a
// parked one. When the harness has a conversation in the worktree the message
// is literally its next turn (claude --continue); otherwise a fresh session
// gets it wrapped in run context.

import { $ } from "../dom.js";
import { App } from "../app.js";

export function openMessageAgent(run, onDone) {
  const live = run.state === "building";
  $("#sheet").innerHTML = `
    <h3>Message the agent</h3>
    <div class="sub">${
      live
        ? "The current run is redirected: the agent picks up your message and continues."
        : "The agent resumes with your message as its next instruction."
    }</div>
    <textarea id="agentmsg" placeholder="e.g. Use the staging credentials. Skip the flaky e2e suite."></textarea>
    <div class="row"><button class="btn" id="msgcancel" style="margin-left:auto">Cancel</button>
      <button class="btn primary" id="msgsend">Send</button></div>
    <div class="adderr" id="msgerr"></div>`;
  $("#scrim").classList.add("show");
  $("#agentmsg").focus();
  $("#msgcancel").onclick = () => $("#scrim").classList.remove("show");
  $("#msgsend").onclick = async () => {
    const message = $("#agentmsg").value.trim();
    if (!message) return;
    $("#msgsend").disabled = true;
    $("#msgsend").textContent = "sending…";
    try {
      await App.call("run.message", { run_id: run.run_id, message });
      $("#scrim").classList.remove("show");
      if (onDone) onDone();
    } catch (e) {
      $("#msgsend").disabled = false;
      $("#msgsend").textContent = "Send";
      $("#msgerr").textContent = e.message;
    }
  };
}

// Freeform message to a plan's planning agent: redirects a live drafting
// session or resumes a parked one (blocked/failed/idle/interrupted). The
// bridge gates non-messageable states (created/plan_review/approved/abandoned);
// callers only offer this button on the messageable arms.
export function openPlanMessage(plan, onDone) {
  const live = plan.state === "drafting";
  $("#sheet").innerHTML = `
    <h3>Message the planning agent</h3>
    <div class="sub">${
      live
        ? "The current planning session is redirected: the agent picks up your message and continues drafting."
        : "The planning agent resumes with your message as its next instruction."
    }</div>
    <textarea id="planmsg" placeholder="e.g. Fold the migration into stage 2. Assume Postgres, not SQLite."></textarea>
    <div class="row"><button class="btn" id="pmsgcancel" style="margin-left:auto">Cancel</button>
      <button class="btn primary" id="pmsgsend">Send</button></div>
    <div class="adderr" id="pmsgerr"></div>`;
  $("#scrim").classList.add("show");
  $("#planmsg").focus();
  $("#pmsgcancel").onclick = () => $("#scrim").classList.remove("show");
  $("#pmsgsend").onclick = async () => {
    const message = $("#planmsg").value.trim();
    if (!message) return;
    $("#pmsgsend").disabled = true;
    $("#pmsgsend").textContent = "sending…";
    try {
      await App.call("plan.message", { plan_id: plan.plan_id, message });
      $("#scrim").classList.remove("show");
      if (onDone) onDone();
    } catch (e) {
      $("#pmsgsend").disabled = false;
      $("#pmsgsend").textContent = "Send";
      $("#pmsgerr").textContent = e.message;
    }
  };
}
