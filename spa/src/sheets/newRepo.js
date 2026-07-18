// Create a brand-new git repo: browse to a location, name it, optionally set a
// remote. `pre` carries field values across the Browse round-trip.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";
import { openBrowser } from "./browser.js";

export function openNewRepo(onDone, pre = {}) {
  $("#sheet").innerHTML = `
    <h3>New repository</h3>
    <div class="sub">Build creates a git repo at the chosen location and registers it.</div>
    <div class="field"><label>Location</label>
      <div class="locrow"><input id="nrloc" style="flex:1" placeholder="(projects folder)" value="${esc(pre.location || "")}" readonly />
        <button class="btn" id="nrbrowse" type="button">Browse…</button></div></div>
    <div class="field"><label>Name</label><input id="nrname" placeholder="my-new-project" style="width:100%" value="${esc(pre.name || "")}" /></div>
    <div class="field"><label>Base branch</label><input id="nrbranch" placeholder="main" style="width:100%" value="${esc(pre.base_branch || "")}" /></div>
    <div class="field"><label>Remote URL (optional)</label><input id="nrremote" placeholder="git@github.com:org/repo.git" style="width:100%" value="${esc(pre.remote || "")}" /></div>
    <div class="row"><button class="btn" id="nrcancel" style="margin-left:auto">Cancel</button><button class="btn primary" id="nrdo">Create</button></div>
    <div class="adderr" id="nrerr"></div>`;
  $("#scrim").classList.add("show");
  $("#nrname").focus();
  const fields = () => ({
    location: $("#nrloc").value,
    name: $("#nrname").value,
    base_branch: $("#nrbranch").value,
    remote: $("#nrremote").value,
  });
  $("#nrcancel").onclick = () => $("#scrim").classList.remove("show");
  $("#nrbrowse").onclick = () => {
    const current = fields();
    openBrowser({
      title: "Choose where to create the repo",
      gitOnly: false,
      onChoose: (path) => openNewRepo(onDone, { ...current, location: path }),
      onCancel: () => openNewRepo(onDone, current),
    });
  };
  $("#nrdo").onclick = async () => {
    const name = $("#nrname").value.trim();
    if (!name) {
      $("#nrerr").textContent = "Enter a name first.";
      return;
    }
    const parent = $("#nrloc").value.trim() || undefined;
    const base_branch = $("#nrbranch").value.trim() || undefined;
    const remote = $("#nrremote").value.trim() || undefined;
    $("#nrdo").disabled = true;
    $("#nrdo").textContent = "creating…";
    $("#nrerr").textContent = "";
    try {
      await App.call("project.create", { name, parent, base_branch, remote });
      $("#scrim").classList.remove("show");
      onDone && onDone();
    } catch (e) {
      $("#nrerr").textContent = e.message;
      $("#nrdo").disabled = false;
      $("#nrdo").textContent = "Create";
    }
  };
}
