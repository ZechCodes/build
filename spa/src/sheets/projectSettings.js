// A project's settings, from the ⋯ menu on any project surface. The bridge holds
// a project's name, path and base branch as facts of the repo it was registered
// from — none of them are editable — so the sheet is the two choices a
// registered project still has: how its work is isolated, and where it pushes.
// The isolation control is mounted, not written here: which isolations exist,
// what they are called and how one is saved are core/isolation.js's facts.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { isolationFieldHtml, mountIsolation, projectIsolationTarget } from "../core/isolation.js";
import { App } from "../app.js";

const field = (label, id, value) =>
  `<div class="field"><label>${esc(label)}</label>
    <input id="${id}" style="width:100%" value="${esc(value || "")}" readonly /></div>`;

const sourcesHtml = (project) => !project.sources?.length ? "" : `<fieldset style="border:0;padding:0;margin:0"><legend>Workspace folders</legend>
  ${project.sources.map((source, index) => `<div class="field"><label for="pssource-${index}">${esc(source.mount || source.name || `Folder ${index + 1}`)}</label>
    <input id="pssource-${index}" style="width:100%" value="${esc(source.path || source.remote || "")}" readonly>
    <div class="dim">${source.is_git === false ? "Folder" : `Git repository${source.base_branch ? ` · ${esc(source.base_branch)}` : ""}`}</div></div>`).join("")}</fieldset>`;

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
      ${sourcesHtml(project)}
      ${isolationFieldHtml()}
      <div class="field"><label>Origin remote</label>
        <input id="psremote" placeholder="git@github.com:org/repo.git" style="width:100%" value="${esc(project.remote || "")}" /></div>
      <div class="row"><button class="btn" id="pscancel" style="margin-left:auto">Close</button>
        <button class="btn primary" id="pssave">Save remote</button></div>
      <div class="adderr" id="pserr"></div>`;
    mountIsolation(sheet, { callRpc, target: projectIsolationTarget(project), settings: project });
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
