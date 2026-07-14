// Freeform message to a task's agent: redirects a live session or resumes a
// parked one. When the harness has a conversation in the worktree the message
// is literally its next turn (claude --continue); otherwise a fresh session
// gets it wrapped in task context.

import { $ } from "../dom.js";
import { App } from "../app.js";

export function openMessageAgent(task, onDone) {
  const live = task.state === "building" || task.state === "planning";
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
      await App.call("task.message", { task_id: task.task_id, message });
      $("#scrim").classList.remove("show");
      if (onDone) onDone();
    } catch (e) {
      $("#msgsend").disabled = false;
      $("#msgsend").textContent = "Send";
      $("#msgerr").textContent = e.message;
    }
  };
}
