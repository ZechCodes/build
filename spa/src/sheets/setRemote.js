// Set (or clear) a project's origin remote, then refresh.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { settingsSheetHtml } from "./settingsSheet.js";
import { writeProjectSetting } from "../core/settingsRecords.js";
import { fieldTraits } from "../core/fieldTraits.js";

/** Opened with the caller of the machine this project is on: the page that
 *  lists the project has already resolved that. */
export function openSetRemote(project, onDone, { callRpc, deviceId = "" }) {
  $("#sheet").innerHTML = settingsSheetHtml({
    title: "Set remote",
    subtitleHtml: `Origin remote for <strong>${esc(project.name)}</strong>. Leave empty to clear.`,
    bodyHtml: `
    <div class="field"><label>Remote URL</label><input id="srurl" placeholder="git@github.com:org/repo.git" style="width:100%" ${fieldTraits("identifier")} value="${esc(project.remote || "")}" /></div>
    <div class="row"><button class="btn" id="srcancel" style="margin-left:auto">Cancel</button><button class="btn primary" id="srdo">Save</button></div>
    <div class="adderr" id="srerr"></div>`,
  });
  $("#scrim").classList.add("show");
  $("#srurl").focus();
  $("#srcancel").onclick = () => $("#scrim").classList.remove("show");
  $("#srdo").onclick = async () => {
    const url = $("#srurl").value.trim();
    $("#srdo").disabled = true;
    $("#srdo").textContent = "saving…";
    $("#srerr").textContent = "";
    try {
      const changed = await callRpc("project.set_remote", { project_id: project.project_id, url });
      if (changed?.project_id) await writeProjectSetting(deviceId, changed);
      $("#scrim").classList.remove("show");
      onDone && onDone();
    } catch (e) {
      $("#srerr").textContent = e.message;
      $("#srdo").disabled = false;
      $("#srdo").textContent = "Save";
    }
  };
}
