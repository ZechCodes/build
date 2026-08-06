// A project's settings, from the ⋯ menu on any project surface. The bridge holds
// a project's name, path and base branch as facts of the repo it was registered
// from — none of them are editable — and exposes exactly one mutation for an
// existing project: project.set_remote. So that is the whole sheet.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";

const field = (label, id, value) =>
  `<div class="field"><label>${esc(label)}</label>
    <input id="${id}" style="width:100%" value="${esc(value || "")}" readonly /></div>`;

export function openProjectSettings(projectId, { callRpc = (method, params) => App.call(method, params) } = {}) {
  const sheet = $("#sheet");
  sheet.innerHTML = `<h3>Project settings</h3><div class="sub">Loading…</div>`;
  $("#scrim").classList.add("show");

  const close = () => $("#scrim").classList.remove("show");

  const paintMissing = (message) => {
    sheet.innerHTML = `<h3>Project settings</h3>
      <div class="sub">${esc(message)}</div>
      <div class="row"><button class="btn" id="pscancel" style="margin-left:auto">Close</button></div>`;
    $("#pscancel").onclick = close;
  };

  const paint = (project) => {
    sheet.innerHTML = `
      <h3>Project settings</h3>
      <div class="sub">Name, location and base branch come from the repository Build was pointed at.</div>
      ${field("Name", "psname", project.name)}
      ${field("Repository path", "pspath", project.path)}
      ${field("Base branch", "psbranch", project.base_branch)}
      <div class="field"><label>Origin remote</label>
        <input id="psremote" placeholder="git@github.com:org/repo.git" style="width:100%" value="${esc(project.remote || "")}" /></div>
      <div class="row"><button class="btn" id="pscancel" style="margin-left:auto">Close</button>
        <button class="btn primary" id="pssave">Save remote</button></div>
      <div class="adderr" id="pserr"></div>`;
    $("#pscancel").onclick = close;
    $("#pssave").onclick = async () => {
      const save = $("#pssave");
      save.disabled = true;
      save.textContent = "saving…";
      $("#pserr").textContent = "";
      try {
        await callRpc("project.set_remote", { project_id: projectId, url: $("#psremote").value.trim() });
        close();
      } catch (e) {
        $("#pserr").textContent = e.message;
        save.disabled = false;
        save.textContent = "Save remote";
      }
    };
  };

  callRpc("project.list")
    .then((listed) => {
      const project = (listed.projects || []).find((candidate) => candidate.project_id === projectId);
      if (project) paint(project);
      else paintMissing("This project is no longer registered on this device.");
    })
    .catch((e) => paintMissing(`Project settings are unavailable: ${e.message}`));
}
