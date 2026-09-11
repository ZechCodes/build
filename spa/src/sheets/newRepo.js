// One entry point for adding a project: open an existing folder, or create a
// repository in this device's configured projects directory.
import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App } from "../app.js";
import { openBrowser } from "./browser.js";

export function openNewRepo(onDone) {
  const sheet = $("#sheet");
  const scrim = $("#scrim");
  const session = App.session;
  const call = App.call;
  let active = true;
  let busy = false;
  const draft = { name: "", remote: "" };
  const visible = (node) => active && node.isConnected && scrim.classList.contains("show");
  const close = () => { active = false; scrim.classList.remove("show"); };
  const callRpc = (method, params) => {
    if (!active || App.session !== session || App.call !== call) {
      return Promise.reject(new Error("The active device changed. Reopen Add project on the device you want."));
    }
    return call(method, params);
  };
  const submit = async (method, params, errorElement, button) => {
    if (busy) return;
    busy = true;
    const anchor = sheet.firstElementChild;
    if (button) button.disabled = true;
    errorElement.textContent = "";
    try {
      const project = await callRpc(method, params);
      if (!visible(anchor) || App.session !== session) return;
      close();
      onDone?.(project);
    } catch (error) {
      if (visible(anchor)) errorElement.textContent = error.message;
    } finally {
      busy = false;
      if (button?.isConnected) button.disabled = false;
    }
  };
  const chooseExisting = async () => {
    const anchor = sheet.firstElementChild;
    const button = $("#nrexisting");
    button.disabled = true;
    try {
      const { projects_dir } = await callRpc("settings.get");
      if (!visible(anchor)) return;
      if (!projects_dir) throw new Error("This device did not return a projects folder.");
      await openBrowser({
        title: "Use an existing project folder",
        gitOnly: false,
        startPath: projects_dir,
        callRpc,
        onCancel: paintChoices,
        onChoose: (path) => submit("project.add", { path }, $("#berr") || $("#nrerr"), $("#choosecur")),
      });
    } catch (error) {
      if (visible(anchor)) $("#nrerr").textContent = error.message;
    } finally {
      if (button.isConnected) button.disabled = false;
    }
  };
  function paintChoices() {
    sheet.innerHTML = `
      <h3>Add project</h3>
      <p class="sub">Use a folder on this device, or create a project in its configured projects folder.</p>
      <div class="row"><button class="btn" id="nrexisting" type="button">Use existing folder…</button>
      <button class="btn primary" id="nrnew" type="button">Create new project</button></div>
      <div class="row"><button class="btn" id="nrcancel" type="button">Cancel</button></div>
      <div class="adderr" id="nrerr" role="status"></div>`;
    $("#nrexisting").onclick = chooseExisting;
    $("#nrnew").onclick = paintCreate;
    $("#nrcancel").onclick = close;
  }
  function paintCreate() {
    sheet.innerHTML = `
      <h3>Create new project</h3>
      <p class="sub">The project will be created in this device's configured projects folder.</p>
      <form id="nrform">
        <div class="field"><label for="nrname">Name</label><input id="nrname" required placeholder="my-project" style="width:100%" value="${esc(draft.name)}" /></div>
        <div class="field"><label for="nrremote">Git remote (optional)</label><input id="nrremote" placeholder="git@github.com:org/repo.git" style="width:100%" value="${esc(draft.remote)}" /></div>
        <div class="row"><button class="btn" id="nrback" type="button">Back</button>
        <button class="btn" id="nrcancel" type="button" style="margin-left:auto">Cancel</button>
        <button class="btn primary" id="nrdo" type="submit">Create project</button></div>
        <div class="adderr" id="nrerr" role="status"></div>
      </form>`;
    $("#nrcancel").onclick = close;
    const remember = () => { draft.name = $("#nrname").value; draft.remote = $("#nrremote").value; };
    $("#nrback").onclick = () => { remember(); paintChoices(); };
    $("#nrform").onsubmit = (event) => {
      event.preventDefault();
      remember();
      const name = draft.name.trim();
      if (!name) { $("#nrerr").textContent = "Enter a name first."; return; }
      const params = { name };
      if (draft.remote.trim()) params.remote = draft.remote.trim();
      void submit("project.create", params, $("#nrerr"), $("#nrdo"));
    };
    $("#nrname").focus();
  }
  scrim.classList.add("show");
  paintChoices();
}
