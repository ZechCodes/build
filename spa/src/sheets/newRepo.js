import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";
import { openBrowser } from "./browser.js";

const inferredName = (value) => (value.trim().replace(/[\\/]+$/, "").replace(/\.git$/i, "").split(/[\\/:]/).pop() || "folder").replace(/[^a-zA-Z0-9._-]+/g, "-");

export function openNewRepo(onDone) {
  const sheet = $("#sheet"), scrim = $("#scrim"), session = App.session, call = App.call;
  const draft = { name: "", sources: [] };
  let active = true, busy = false, serial = 0, version = 0, projectsDir;
  const visible = (node) => active && node?.isConnected && scrim.classList.contains("show");
  const close = () => { active = false; version += 1; scrim.classList.remove("show"); };
  const callRpc = (method, params) => active && App.session === session && App.call === call
    ? call(method, params)
    : Promise.reject(new Error("The active device changed. Reopen Add project on the device you want."));
  const remember = () => {
    if (sheet.querySelector("#nrname")) draft.name = $("#nrname").value;
  };
  const uniqueName = (value, except) => {
    const base = inferredName(value), used = new Set(draft.sources.filter((source) => source !== except).map((source) => source.name.trim().toLowerCase()));
    let name = base;
    for (let n = 2; used.has(name.toLowerCase()); n += 1) name = `${base}-${n}`;
    return name;
  };
  const addSource = (kind) => { const source = { id: ++serial, kind, path: "", remote: "", name: "", base_branch: "", automaticName: true }; draft.sources.push(source); return source; };
  const submit = async (params) => {
    if (busy) return;
    busy = true;
    const anchor = sheet.firstElementChild;
    sheet.querySelectorAll("button,input").forEach((node) => { node.disabled = true; });
    $("#nrdo").setAttribute("aria-busy", "true"); $("#nrerr").textContent = "";
    try {
      const project = await callRpc("project.create", params);
      if (!visible(anchor) || App.session !== session) return;
      close(); onDone?.(project);
    } catch (error) { if (visible(anchor)) $("#nrerr").textContent = error.message; }
    finally {
      busy = false;
      if (visible(anchor)) {
        sheet.querySelectorAll("button,input").forEach((node) => { node.disabled = false; });
        $("#nrdo")?.removeAttribute("aria-busy");
      }
    }
  };
  const sourceHtml = (source, index) => `<fieldset data-source-row="${source.id}" style="border:1px solid var(--line);border-radius:8px;margin:10px 0;padding:10px"><legend>Folder ${index + 1}</legend>
    ${source.kind === "remote" ? `<div class="field"><label for="nrsource-${source.id}">Git remote URL</label><input id="nrsource-${source.id}" data-source-value="${source.id}" value="${esc(source.remote)}" placeholder="git@github.com:org/repo.git"></div>` : `<div class="field"><span id="nrsource-label-${source.id}">Selected folder</span><div class="browse-path" aria-labelledby="nrsource-label-${source.id}">${source.path ? esc(source.path) : "No folder selected"}</div><button class="btn" type="button" data-choose-source="${source.id}">Choose folder…</button></div>`}
    <div class="field"><label for="nrmount-${source.id}">Folder name</label><input id="nrmount-${source.id}" data-source-name="${source.id}" value="${esc(source.name)}"></div>
    <div class="field"><label for="nrbranch-${source.id}">Base branch (optional)</label><input id="nrbranch-${source.id}" data-source-branch="${source.id}" value="${esc(source.base_branch)}" placeholder="main"></div>
    <button class="btn" type="button" data-remove-source="${source.id}" aria-label="Remove folder ${index + 1}">Remove</button></fieldset>`;
  const sourceError = (source, names) => {
    if (!source[source.kind].trim()) return [source.kind === "remote" ? "Enter a Git remote URL." : "Choose a folder.", source.kind === "remote" ? `#nrsource-${source.id}` : `[data-choose-source="${source.id}"]`];
    const name = source.name.trim();
    if (!name) return ["Enter a folder name.", `#nrmount-${source.id}`];
    if (/[\\/]/.test(name) || name === "." || name === "..") return ["Use a single folder name without / or \\.", `#nrmount-${source.id}`];
    if (names.has(name.toLowerCase())) return [`Folder names must be unique. “${name}” is used more than once.`, `#nrmount-${source.id}`];
    names.add(name.toLowerCase());
    return null;
  };
  const invalidSource = () => {
    if (!draft.name.trim()) return ["Enter a project name.", "#nrname"];
    if (!draft.sources.length) return ["Add at least one workspace folder.", "#nraddfolder"];
    const names = new Set();
    for (const source of draft.sources) {
      const error = sourceError(source, names);
      if (error) return error;
    }
    return null;
  };
  const browseFor = async (sourceId) => {
    remember(); const requestVersion = ++version;
    if (!draft.sources.some((source) => source.id === sourceId)) return;
    sheet.innerHTML = `<h3>Choose folder</h3><p class="sub">Choose a folder to add to ${esc(draft.name.trim() || "this project")}.</p><div id="nrbrowser"></div><button class="btn" id="nrback" type="button">Back</button><div class="adderr" id="nrerr" role="status"></div>`;
    $("#nrback").onclick = paint;
    try {
      if (projectsDir === undefined) projectsDir = (await callRpc("settings.get")).projects_dir;
      const host = $("#nrbrowser");
      if (requestVersion !== version || !visible(host)) return;
      if (!projectsDir) throw new Error("This device did not return a projects folder.");
      await openBrowser({ title: "Choose a workspace folder", gitOnly: false, allowCreateDirectory: true, startPath: projectsDir, callRpc, container: host, onChoose: (path) => {
        if (requestVersion !== version || !visible(host)) return;
        const source = draft.sources.find((item) => item.id === sourceId);
        if (!source) return;
        source.path = path; if (source.automaticName) source.name = uniqueName(path, source); paint();
      }});
    } catch (error) { if (requestVersion === version && active) $("#nrerr").textContent = error.message; }
  };
  const paintSources = () => {
    version += 1;
    sheet.innerHTML = `<h3>Add project</h3><p class="sub">Add Git remotes or folders from this device. Each becomes a folder in the project.</p><form id="nrform">
      <div class="field"><label for="nrname">Project name</label><input id="nrname" required value="${esc(draft.name)}"></div>
      <fieldset style="border:0;padding:0;margin:0"><legend>Workspace folders</legend><div id="nrsources">${draft.sources.map(sourceHtml).join("")}</div><div class="row"><button class="btn" id="nraddfolder" type="button">Add folder</button><button class="btn" id="nraddremote" type="button">Add Git remote</button></div></fieldset>
      <div class="row"><button class="btn" id="nrcancel" type="button" style="margin-left:auto">Cancel</button><button class="btn primary" id="nrdo" type="submit">Create project</button></div><div class="adderr" id="nrerr" role="status" aria-live="polite"></div></form>`;
    $("#nrcancel").onclick = close;
    $("#nraddfolder").onclick = () => { remember(); void browseFor(addSource("path").id); };
    $("#nraddremote").onclick = () => { remember(); const source = addSource("remote"); paint(); $(`#nrsource-${source.id}`)?.focus(); };
    sheet.querySelectorAll("[data-source-value]").forEach((input) => input.oninput = () => { const source = draft.sources.find((item) => item.id === Number(input.dataset.sourceValue)); source.remote = input.value; if (source.automaticName) { source.name = uniqueName(input.value, source); $(`#nrmount-${source.id}`).value = source.name; } });
    sheet.querySelectorAll("[data-source-name]").forEach((input) => input.oninput = () => { const source = draft.sources.find((item) => item.id === Number(input.dataset.sourceName)); source.name = input.value; source.automaticName = false; });
    sheet.querySelectorAll("[data-source-branch]").forEach((input) => input.oninput = () => { draft.sources.find((item) => item.id === Number(input.dataset.sourceBranch)).base_branch = input.value; });
    sheet.querySelectorAll("[data-choose-source]").forEach((button) => button.onclick = () => void browseFor(Number(button.dataset.chooseSource)));
    sheet.querySelectorAll("[data-remove-source]").forEach((button) => button.onclick = () => { remember(); draft.sources = draft.sources.filter((source) => source.id !== Number(button.dataset.removeSource)); paint(); $("#nraddfolder").focus(); });
    $("#nrform").onsubmit = (event) => { event.preventDefault(); remember(); const invalid = invalidSource(); if (invalid) { $("#nrerr").textContent = invalid[0]; sheet.querySelector(invalid[1])?.focus(); return; } const sources = draft.sources.map((source) => ({ [source.kind]: source[source.kind].trim(), name: source.name.trim(), ...(source.base_branch.trim() ? { base_branch: source.base_branch.trim() } : {}) })); void submit({ name: draft.name.trim(), sources }); };
    $("#nrname").focus();
  };
  function paint() { if (active) paintSources(); }
  scrim.classList.add("show"); paint();
}
