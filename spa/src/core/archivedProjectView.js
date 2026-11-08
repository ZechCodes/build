// One archive notice and mutation lock for project, task and review surfaces.
// Both initial paint and later transitions read cache records; RPC refusals
// remain the bridge's exact sentence, including an archive racing a click.
import { readCached, subscribeCache } from "./localCache.js";
import { projectIsArchived } from "./archivedProjectModel.js";
import { restoreEnvironmentProject } from "./environmentWrites.js";
import { readEnvironmentSupport } from "./environmentSupport.js";
import { esc } from "./text.js";

const mutationSelectors = [
  "#tb-verb button", "#init-git", "[data-init-git]", "[data-workspace-action]", ".dcmt", ".pcx", ".cscancel", "[data-project-sync-base]", ".gittoolbar button", ".gtstash button", ".csbox button", ".gitdiscard", "[data-ref]", ".fcmt", "textarea", "input:not([type=search])", "[contenteditable=true]", "[data-project-create]", "[data-workspace-reclaim]",
  ".fupload-action", "[data-bubble=add]", ".rail-newagent select", "[data-clear-start]", ".composer-chip-remove", ".composer-send", ".rail-remove", ".rail-harness-choice", ".rail-watch", ".file-save", "[data-upload-action]", ".fupload-new button", "[data-task-state]", "[data-task-assign]", "#task-status", "#task-priority", "#task-labels", "[data-task-index]",
  "[data-review-comment]", "[data-review-verdict]", "[data-review-comment-batch]", "[data-review-reply]", "[data-review-reviewer]", "[data-review-refresh]",
  "[data-review-request-legacy]", "[data-review-close-legacy]", "[data-review-reopen]", "[data-review-repair]", "[data-review-reclaim]",
  "[data-review-retry-delivery]", "[data-review-retry]", "[data-review-recover]", "[data-review-draft-edit]", "[data-review-draft-remove]",
  "[data-pr-merge-source] select", "[data-review-controls] button", "[data-review-git-actions] button", "[data-review-close] button",
  "[data-task-waiting-on] button", "[data-task-waiting-on] select", "form button[type=submit]", ".composer-attach",
  "[data-task-create]", "[data-task-new]", "[data-new-task]", "[data-task-edit]", "[data-task-delete]", "[data-file-save]",
].join(",");

function mutationControl(node) {
  return node.matches(mutationSelectors) || node.closest("[data-archived-mutation]");
}

export function mountArchivedProjectView(host, { deviceId, projectId, callRpc, archiveMutationHosts = [] }) {
  host.setAttribute("data-project-archive-state", "unknown");
  let active = true, project = null, support = {}, refusal = "", reading = 0;
  let banner = null;
  const locked = new Map();
  const hosts = [host, ...archiveMutationHosts.filter(Boolean)];
  const contains = control => hosts.some(root => root.contains(control));
  const refuseBoard = event => {
    if (!projectIsArchived(project)) return;
    if (event.type !== "keydown" && Array.from(event.dataTransfer?.types || []).includes("Files")) { event.preventDefault(); event.stopImmediatePropagation(); return; }
    if (!event.target.closest("[data-task], [data-column-drop]")) return;
    if (event.type === "keydown" && !["ArrowLeft", "ArrowRight"].includes(event.key)) return;
    event.preventDefault(); event.stopImmediatePropagation();
  };
  const refuseEvent = event => {
    if (!projectIsArchived(project)) return;
    const target = event.target.closest(mutationSelectors);
    if (target && contains(target)) { event.preventDefault(); event.stopImmediatePropagation(); }
  };
  for (const root of hosts) for (const event of ["click", "change", "submit"]) root.addEventListener(event, refuseEvent, true);
  for (const event of ["dragstart", "drop", "keydown"]) host.addEventListener(event, refuseBoard, true);
  function lockControls() {
    for (const control of locked.keys()) if (!contains(control)) locked.delete(control);
    hosts.flatMap(root => [...root.querySelectorAll("button, input, select, textarea, [contenteditable], .pcx")]).forEach(control => {
      const shouldLock = projectIsArchived(project) && (locked.has(control) || mutationControl(control));
      if (shouldLock) lock(control);
      else if (locked.has(control)) unlock(control);
    });
  }
  function lock(control) {
    if (!locked.has(control)) locked.set(control, { disabled: control.disabled, editable: control.getAttribute("contenteditable"),
      ariaDisabled: control.getAttribute("aria-disabled"), tabIndex: control.getAttribute("tabindex") });
    if (control.hasAttribute("contenteditable") && control.getAttribute("contenteditable") !== "false") control.setAttribute("contenteditable", "false");
    if ("disabled" in control) {
      if (!control.disabled) control.disabled = true;
    } else {
      control.setAttribute("aria-disabled", "true");
      control.setAttribute("tabindex", "-1");
    }
  }
  const restoreAttribute = (control, name, value) => value === null ? control.removeAttribute(name) : control.setAttribute(name, value);
  function unlock(control) {
    const before = locked.get(control);
    if ("disabled" in control) control.disabled = before.disabled;
    if (before.editable !== null) control.setAttribute("contenteditable", before.editable);
    restoreAttribute(control, "aria-disabled", before.ariaDisabled);
    restoreAttribute(control, "tabindex", before.tabIndex);
    locked.delete(control);
  }
  function publishArchiveState(archived) {
    const state = archived ? "archived" : "active";
    if (host.getAttribute("data-project-archive-state") !== state) host.setAttribute("data-project-archive-state", state);
  }
  function paint() {
    if (!active) return;
    const archived = projectIsArchived(project);
    publishArchiveState(archived);
    if (!archived && !locked.size && !banner && !refusal) return;
    if (archived || refusal) paintBanner(archived);
    else { banner?.remove(); banner = null; }
    lockControls();
  }
  function paintBanner(archived) {
    if (!banner?.isConnected) {
      banner = document.createElement("aside"); banner.className = "panel"; banner.setAttribute("data-project-archived", ""); banner.setAttribute("role", "status");
      host.prepend(banner);
    }
    const html = `${archived ? `<p>${esc(project.name || "This project")} is archived. Restore it to make changes.</p>` : ""}
      ${archived && support.archive ? '<button class="btn" data-project-restore>Restore</button>' : ""}<p class="adderr" data-project-archive-error role="alert">${esc(refusal)}</p>`;
    if (banner.dataset.paint === html) return;
    banner.dataset.paint = html; banner.innerHTML = html;
    const restore = banner.querySelector("[data-project-restore]");
    if (restore) restore.onclick = async () => {
      restore.disabled = true;
      try { await restoreEnvironmentProject(deviceId, projectId, callRpc); refusal = ""; await read(); }
      catch (error) { refusal = error.message; banner.dataset.paint = ""; paint(); }
    };
  }
  async function read() {
    const ticket = ++reading;
    const [rows, settings, actions] = await Promise.all([
      readCached({ deviceId, entityId: "", kind: "projects" }),
      readCached({ deviceId, entityId: projectId, kind: "project-settings" }), readEnvironmentSupport(deviceId),
    ]);
    if (!active || ticket !== reading) return;
    project = rows?.value?.find(row => (row.project_id || row.id) === projectId) || settings?.value || null;
    support = actions; paint();
  }
  const unsubscribe = subscribeCache({ deviceId }, address => {
    if (["projects", "project-settings", "environment-support"].includes(address.kind)) void read();
  });
  const observer = new MutationObserver(paint);
  for (const root of hosts) observer.observe(root, { childList: true, subtree: true, attributes: true, attributeFilter: ["disabled", "contenteditable"] });
  void read();
  return {
    callRpc: async (...args) => {
      try { return await callRpc(...args); }
      catch (error) {
        if ((error.code || error.error_code) === "project_archived") { refusal = error.message; paint(); }
        throw error;
      }
    },
    dispose() { for (const event of ["dragstart", "drop", "keydown"]) host.removeEventListener(event, refuseBoard, true); for (const root of hosts) for (const event of ["click", "change", "submit"]) root.removeEventListener(event, refuseEvent, true); active = false; unsubscribe(); observer.disconnect(); host.removeAttribute("data-project-archive-state"); for (const control of locked.keys()) unlock(control); banner?.remove(); },
  };
}
