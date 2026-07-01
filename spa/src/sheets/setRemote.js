// Set (or clear) a project's origin remote, then refresh.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";

export function openSetRemote(project, onDone) {
  $("#sheet").innerHTML = `
    <h3>Set remote</h3>
    <div class="sub">Origin remote for <strong>${esc(project.name)}</strong>. Leave empty to clear.</div>
    <div class="field"><label>Remote URL</label><input id="srurl" placeholder="git@github.com:org/repo.git" style="width:100%" value="${esc(project.remote || "")}" /></div>
    <div class="row"><button class="btn" id="srcancel" style="margin-left:auto">Cancel</button><button class="btn primary" id="srdo">Save</button></div>
    <div class="adderr" id="srerr"></div>`;
  $("#scrim").classList.add("show");
  $("#srurl").focus();
  $("#srcancel").onclick = () => $("#scrim").classList.remove("show");
  $("#srdo").onclick = async () => {
    const url = $("#srurl").value.trim();
    $("#srdo").disabled = true;
    $("#srdo").textContent = "saving…";
    $("#srerr").textContent = "";
    try {
      await App.call("project.set_remote", { project_id: project.project_id, url });
      $("#scrim").classList.remove("show");
      onDone && onDone();
    } catch (e) {
      $("#srerr").textContent = e.message;
      $("#srdo").disabled = false;
      $("#srdo").textContent = "Save";
    }
  };
}
