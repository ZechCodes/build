// Clone a remote into the projects folder, then refresh.

import { $ } from "../dom.js";
import { App } from "../app.js";

export function openClone(onDone) {
  $("#sheet").innerHTML = `
    <h3>Clone a repository</h3>
    <div class="sub">Build clones it into your projects folder and registers it.</div>
    <div class="field"><label>Remote URL</label><input id="clurl" class="path" placeholder="https://github.com/org/repo.git or git@github.com:org/repo.git" style="width:100%" /></div>
    <div class="field"><label>Folder name (optional)</label><input id="clname" class="path" placeholder="defaults to the repo name" style="width:100%" /></div>
    <div class="row"><button class="btn" id="clcancel" style="margin-left:auto">Cancel</button><button class="btn primary" id="cldo">Clone</button></div>
    <div class="adderr" id="clerr"></div>`;
  $("#scrim").classList.add("show");
  $("#clurl").focus();
  $("#clcancel").onclick = () => $("#scrim").classList.remove("show");
  $("#cldo").onclick = async () => {
    const url = $("#clurl").value.trim();
    if (!url) return;
    const name = $("#clname").value.trim() || undefined;
    $("#cldo").disabled = true;
    $("#cldo").textContent = "cloning…";
    $("#clerr").textContent = "";
    try {
      await App.call("project.clone", { url, name });
      $("#scrim").classList.remove("show");
      onDone && onDone();
    } catch (e) {
      $("#clerr").textContent = e.message;
      $("#cldo").disabled = false;
      $("#cldo").textContent = "Clone";
    }
  };
}
