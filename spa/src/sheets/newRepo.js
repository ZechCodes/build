import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { openBrowser } from "./browser.js";

const TABS = ["create", "existing"];

/** Opened with the caller of the machine the project is going on, and that
 *  machine's name: whoever opens the sheet has already resolved which one that
 *  is, so nothing here asks. A caller whose machine goes away refuses on its
 *  own, in that machine's words. */
export function openNewRepo(onDone, { callRpc, deviceName }) {
  const sheet = $("#sheet");
  const scrim = $("#scrim");
  const title = `New repository on ${deviceName}`;
  const draft = { name: "", remote: "", path: "" };
  let active = true;
  let busy = false;
  let tab = "create";
  let renderVersion = 0;
  let projectsDir;

  const visible = (node) => active && node.isConnected && scrim.classList.contains("show");
  const close = () => {
    active = false;
    renderVersion += 1;
    scrim.classList.remove("show");
  };
  const rememberCreate = () => {
    const name = sheet.querySelector("#nrname");
    const remote = sheet.querySelector("#nrremote");
    if (name) draft.name = name.value;
    if (remote) draft.remote = remote.value;
  };
  const tabsHtml = () =>
    `<div class="segmented create-tabs" role="tablist" aria-label="Project source">${TABS.map(
      (kind) =>
        `<button class="btn seg${kind === tab ? " primary" : ""}" type="button" role="tab" aria-selected="${kind === tab}" tabindex="${kind === tab ? "0" : "-1"}" data-project-tab="${kind}">${kind === "create" ? "Create new" : "Existing folder"}</button>`,
    ).join("")}</div>`;
  const submit = async (method, params, errorElement, button) => {
    if (busy) return;
    busy = true;
    const anchor = sheet.firstElementChild;
    button.disabled = true;
    sheet.querySelectorAll("[data-project-tab]").forEach((tabButton) => { tabButton.disabled = true; });
    errorElement.textContent = "";
    try {
      const project = await callRpc(method, params);
      if (!visible(anchor)) return;
      close();
      onDone?.(project);
    } catch (error) {
      if (visible(anchor)) errorElement.textContent = error.message;
    } finally {
      busy = false;
      if (button.isConnected) button.disabled = false;
      if (anchor.isConnected) sheet.querySelectorAll("[data-project-tab]").forEach((tabButton) => { tabButton.disabled = false; });
    }
  };
  const bindTabs = () => {
    const buttons = [...sheet.querySelectorAll("[data-project-tab]")];
    const activate = (kind, focus = false) => {
      if (kind === tab || busy) return;
      rememberCreate();
      tab = kind;
      paint();
      if (focus) sheet.querySelector(`[data-project-tab="${kind}"]`)?.focus();
    };
    buttons.forEach((button, index) => {
      button.onclick = () => activate(button.dataset.projectTab);
      button.onkeydown = (event) => {
        if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
        event.preventDefault();
        const offset = event.key === "ArrowRight" ? 1 : -1;
        const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : (index + offset + buttons.length) % buttons.length;
        activate(buttons[next].dataset.projectTab, true);
      };
    });
  };
  const paintCreate = () => {
    sheet.innerHTML = `<h3>${esc(title)}</h3>${tabsHtml()}
      <p class="sub">The project will be created in this device's configured projects folder.</p>
      <form id="nrform">
        <div class="field"><label for="nrname">Name</label><input id="nrname" required placeholder="my-project" value="${esc(draft.name)}" /></div>
        <div class="field"><label for="nrremote">Git remote (optional)</label><input id="nrremote" placeholder="git@github.com:org/repo.git" value="${esc(draft.remote)}" /></div>
        <div class="row"><button class="btn" id="nrcancel" type="button" style="margin-left:auto">Cancel</button><button class="btn primary" id="nrdo" type="submit">Create project</button></div>
        <div class="adderr" id="nrerr" role="status"></div>
      </form>`;
    bindTabs();
    $("#nrcancel").onclick = close;
    $("#nrform").onsubmit = (event) => {
      event.preventDefault();
      rememberCreate();
      const name = draft.name.trim();
      if (!name) {
        $("#nrerr").textContent = "Enter a name first.";
        return;
      }
      const params = { name };
      if (draft.remote.trim()) params.remote = draft.remote.trim();
      void submit("project.create", params, $("#nrerr"), $("#nrdo"));
    };
    $("#nrname").focus();
  };
  const loadExistingBrowser = async (version) => {
    const host = sheet.querySelector("#nrbrowser");
    try {
      if (projectsDir === undefined) {
        const settings = await callRpc("settings.get");
        projectsDir = settings.projects_dir;
      }
      if (version !== renderVersion || !visible(host)) return;
      if (!projectsDir) throw new Error("This device did not return a projects folder.");
      await openBrowser({
        title: "Choose an existing project folder",
        gitOnly: false,
        startPath: projectsDir,
        callRpc,
        container: host,
        onChoose: (path) => {
          if (version !== renderVersion || !visible(host)) return;
          draft.path = path;
          sheet.querySelector("#nrexistingpath").textContent = path;
          sheet.querySelector("#nrdo").disabled = false;
          sheet.querySelector("#nrerr").textContent = "";
        },
      });
    } catch (error) {
      if (version === renderVersion && visible(host)) sheet.querySelector("#nrerr").textContent = error.message;
    }
  };
  const paintExisting = () => {
    const version = ++renderVersion;
    sheet.innerHTML = `<h3>${esc(title)}</h3>${tabsHtml()}
      <p class="sub">Choose a folder on this device to add as a project.</p>
      <div id="nrbrowser"></div>
      <div class="field"><label>Selected folder</label><div class="browse-path" id="nrexistingpath">${draft.path ? esc(draft.path) : "No folder selected"}</div></div>
      <div class="row"><button class="btn" id="nrcancel" type="button" style="margin-left:auto">Cancel</button><button class="btn primary" id="nrdo" type="button"${draft.path ? "" : " disabled"}>Add project</button></div>
      <div class="adderr" id="nrerr" role="status"></div>`;
    bindTabs();
    $("#nrcancel").onclick = close;
    $("#nrdo").onclick = () => void submit("project.add", { path: draft.path }, $("#nrerr"), $("#nrdo"));
    void loadExistingBrowser(version);
  };
  function paint() {
    renderVersion += 1;
    if (tab === "create") paintCreate();
    else paintExisting();
  }

  scrim.classList.add("show");
  paint();
}
