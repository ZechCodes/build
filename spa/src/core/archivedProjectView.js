// One archive notice and mutation lock for project, task and review surfaces.
// Both initial paint and later transitions read cache records; RPC refusals
// remain the bridge's exact sentence, including an archive racing a click.
import { readCached, subscribeCache } from "./localCache.js";
import { projectIsArchived } from "./archivedProjectModel.js";
import { restoreEnvironmentProject } from "./environmentWrites.js";
import { readEnvironmentSupport } from "./environmentSupport.js";
import { esc } from "./text.js";

const mutationSelectors = [
  "textarea", "input:not([type=search])", "[contenteditable=true]", "[data-project-create]", "[data-workspace-reclaim]",
  "[data-task-state]", "[data-task-assign]", "#task-status", "#task-priority", "#task-labels", "[data-task-index]",
  "[data-review-verdict]", "[data-review-comment-batch]", "[data-review-reply]", "[data-review-reviewer]", "[data-review-refresh]",
  "[data-review-request-legacy]", "[data-review-close-legacy]", "[data-review-reopen]", "[data-review-repair]", "[data-review-reclaim]",
  "[data-review-retry-delivery]", "[data-review-retry]", "[data-review-recover]", "[data-review-draft-edit]", "[data-review-draft-remove]",
  "[data-review-controls] button", "[data-review-git-actions] button", "[data-review-close] button",
  "[data-task-waiting-on] button", "[data-task-waiting-on] select", "form button[type=submit]", ".composer-attach",
  "[data-task-create]", "[data-new-task]", "[data-task-edit]", "[data-task-delete]", "[data-file-save]",
].join(",");

function mutationControl(node) {
  return node.matches(mutationSelectors) || node.closest("[data-archived-mutation]");
}

export function mountArchivedProjectView(host, { deviceId, projectId, callRpc }) {
  let active = true, project = null, support = {}, refusal = "", reading = 0;
  let banner = null;
  const locked = new Map();
  function lockControls() {
    host.querySelectorAll("button, input, select, textarea, [contenteditable]").forEach(control => {
      const shouldLock = projectIsArchived(project) && mutationControl(control);
      if (shouldLock) {
        if (!locked.has(control)) locked.set(control, { disabled: control.disabled, editable: control.getAttribute("contenteditable") });
        if (control.hasAttribute("contenteditable") && control.getAttribute("contenteditable") !== "false") control.setAttribute("contenteditable", "false");
        if ("disabled" in control && !control.disabled) control.disabled = true;
      } else if (locked.has(control)) unlock(control);
    });
  }
  function unlock(control) {
    const before = locked.get(control);
    if ("disabled" in control) control.disabled = before.disabled;
    if (before.editable !== null) control.setAttribute("contenteditable", before.editable);
    locked.delete(control);
  }
  function paint() {
    if (!active) return;
    const archived = projectIsArchived(project);
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
  observer.observe(host, { childList: true, subtree: true, attributes: true, attributeFilter: ["disabled", "contenteditable"] });
  void read();
  return {
    callRpc: async (...args) => {
      try { return await callRpc(...args); }
      catch (error) {
        if ((error.code || error.error_code) === "project_archived") { refusal = error.message; paint(); }
        throw error;
      }
    },
    dispose() { active = false; unsubscribe(); observer.disconnect(); for (const control of locked.keys()) unlock(control); banner?.remove(); },
  };
}
