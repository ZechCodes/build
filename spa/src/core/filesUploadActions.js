import { esc } from "./text.js";
import { parentPath } from "./fileTreeModel.js";
import { notifyError } from "./notify.js";
import "../styles/fileUploadActions.css";

const actionButton = (action, title, mark) => `<button type="button" class="iconbtn" data-upload-action="${action}" aria-label="${title}" title="${title}">${mark}</button>`;
const actionsHtml = (support) => `<span class="fupload-directory-actions">${support.uploads ? actionButton("upload", "Upload files", "↑") : ""}${support.createDirectory ? actionButton("folder", "New folder", "+") : ""}</span>`;
const fileDrag = (event) => Array.from(event.dataTransfer?.types || []).includes("Files");
const rootIdOf = (row) => row.closest("[data-root]")?.dataset.root ?? row.dataset.rootHead;
const directoryRows = (host) => [...host.querySelectorAll('[data-kind="dir"], [data-root-head], [data-upload-root]')];
const interactive = (event) => event.target.closest?.("[data-upload-action], .fupload-folder, .fupload-input");
const destination = (root, parent) => [root.label, parent].filter(Boolean).join(" / ") || "/";
const validFolderName = (name) => name && name !== "." && name !== ".." && !/[\\/\0]/.test(name);

/** Directory affordances decorate existing rows; drag feedback only changes a
 * class and an overlay attribute, preserving the tree's elements and geometry. */
export function mountFilesUploadActions(host, { roots, capabilities, uploads, callRpc, onCreated }) {
  let support = capabilities;
  let disposed = false;
  let dropRow = null;
  let selectedTarget = null;
  let draft = null;
  const input = document.createElement("input");
  input.className = "fupload-input";
  input.type = "file";
  input.multiple = true;
  input.hidden = true;
  host.append(input);

  const rootOf = (row) => roots.find((root) => String(root.id) === rootIdOf(row)) || roots[0];
  const parentOf = (row) => row.dataset.kind === "dir" ? row.dataset.path : "";
  const targetOfRow = (row) => ({ root: rootOf(row), parent: parentOf(row), row });
  const rootHead = (root) => directoryRows(host).find((row) => rootOf(row) === root && !row.dataset.path);
  const parentRow = (root, path) => directoryRows(host).find((row) => rootOf(row) === root && row.dataset.path === path) || rootHead(root);

  const targetOf = (event) => {
    const row = event.target.closest?.(".frow");
    if (row?.dataset.kind === "dir") return targetOfRow(row);
    const root = row ? rootOf(row) : rootOf(event.target.closest?.("[data-root]") || host);
    const parent = row?.dataset.path ? parentPath(row.dataset.path) : "";
    return { root, parent, row: parentRow(root, parent) };
  };

  const clearDrop = () => {
    dropRow?.classList.remove("fupload-drop");
    if (dropRow) delete dropRow.dataset.uploadHint;
    dropRow = null;
  };
  const markDrop = (target) => {
    if (dropRow === target.row) return;
    clearDrop();
    dropRow = target.row;
    if (!dropRow) return;
    dropRow.classList.add("fupload-drop");
    dropRow.dataset.uploadHint = `Drop to upload into ${target.parent.split("/").at(-1) || target.root.label || "Files"}`;
  };

  const uploadTarget = ({ root, parent }) => ({ scope: { ...root.scope }, rootId: root.id, parent, destination: destination(root, parent), callRpc });
  const createdDirectory = ({ rootId, parent, path }) => {
    const root = roots.find((candidate) => candidate.id === rootId);
    if (!disposed && root) onCreated(root, parent, path);
  };
  const onDragOver = (event) => {
    if (!fileDrag(event)) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = support.uploads ? "copy" : "none";
    if (support.uploads) markDrop(targetOf(event));
  };
  const onDragLeave = (event) => {
    if (!host.contains(event.relatedTarget)) clearDrop();
  };
  const onDrop = (event) => {
    if (!fileDrag(event)) return;
    event.preventDefault();
    const target = targetOf(event);
    clearDrop();
    if (!support.uploads) return notifyError("Update the bridge to upload files");
    void uploads.enqueueDrop({ ...uploadTarget(target), dataTransfer: event.dataTransfer, onDirectory: createdDirectory })
      .catch((error) => notifyError("Could not upload files", error.message));
  };

  const closeDraft = () => {
    const row = draft?.target.row;
    draft?.element.remove();
    draft = null;
    if (row?.isConnected) row.focus();
  };
  const showDraftError = (error) => {
    draft.error.textContent = error;
    draft.field.disabled = false;
    draft.field.focus();
  };
  const createFolder = async () => {
    const current = draft;
    const name = current.field.value;
    if (!validFolderName(name)) return showDraftError("Enter a folder name without slashes.");
    current.field.disabled = true;
    current.error.textContent = "";
    const { root, parent } = current.target;
    try {
      const result = await callRpc("fs.createDirectory", { ...root.scope, parent, name });
      if (disposed || draft !== current) return;
      closeDraft();
      onCreated(root, parent, result.path);
    } catch (error) {
      if (!disposed && draft === current) showDraftError(error.message || "Could not create folder.");
    }
  };
  const newFolder = (target) => {
    closeDraft();
    const element = document.createElement("div");
    element.className = "fupload-folder";
    element.style.setProperty("--depth", Number(target.row.style.getPropertyValue("--depth") || 0) + 1);
    element.innerHTML = `<input type="text" aria-label="New folder name" placeholder="Folder name" autocomplete="off"><span class="fupload-folder-error" role="alert"></span>`;
    target.row.after(element);
    draft = { target, element, field: element.querySelector("input"), error: element.querySelector("span") };
    draft.field.focus();
  };
  const onClick = (event) => {
    if (!interactive(event)) return;
    event.stopPropagation();
    const button = event.target.closest("[data-upload-action]");
    if (!button) return;
    const target = targetOfRow(button.closest(".frow"));
    if (button.dataset.uploadAction === "folder") return newFolder(target);
    selectedTarget = target;
    input.value = "";
    input.click();
  };
  const onKeyDown = (event) => {
    if (!interactive(event)) return;
    event.stopPropagation();
    if (!event.target.closest(".fupload-folder")) return;
    if (event.key === "Escape") { event.preventDefault(); closeDraft(); }
    if (event.key === "Enter") { event.preventDefault(); if (!draft.field.disabled) void createFolder(); }
  };
  const onChange = () => {
    if (selectedTarget && input.files.length) uploads.enqueue({ ...uploadTarget(selectedTarget), files: Array.from(input.files) });
  };

  const ensureRoot = () => {
    if (roots.length !== 1 || roots[0].id !== null || host.querySelector("[data-upload-root]")) return;
    const root = document.createElement("div");
    root.className = "frow fdir fupload-root";
    root.dataset.uploadRoot = "";
    root.setAttribute("role", "group");
    root.setAttribute("aria-label", "Files root");
    root.tabIndex = 0;
    root.innerHTML = `<span class="fname">${esc(roots[0].label || "Files")}</span>`;
    host.prepend(root);
  };
  const decorateRow = (row, markup) => {
    const held = row.querySelector(".fupload-directory-actions");
    if (held?.outerHTML === markup) return;
    held?.remove();
    if (support.uploads || support.createDirectory) row.insertAdjacentHTML("beforeend", markup);
  };
  const restoreDraft = () => {
    if (!draft || draft.element.isConnected) return;
    draft.target.row = parentRow(draft.target.root, draft.target.parent);
    draft.target.row?.after(draft.element);
  };
  const decorate = () => {
    if (disposed) return;
    if (support.uploads || support.createDirectory) ensureRoot();
    const markup = actionsHtml(support);
    directoryRows(host).forEach((row) => decorateRow(row, markup));
    // A cache listing can repaint while a name is being typed. Keep that field
    // at its level without replacing it or losing the draft.
    restoreDraft();
  };
  const observer = new MutationObserver(decorate);
  observer.observe(host, { childList: true, subtree: true });
  decorate();
  host.addEventListener("click", onClick, true);
  host.addEventListener("keydown", onKeyDown, true);
  host.addEventListener("dragover", onDragOver);
  host.addEventListener("dragenter", onDragOver);
  host.addEventListener("dragleave", onDragLeave);
  host.addEventListener("drop", onDrop);
  input.addEventListener("change", onChange);
  return {
    setCapabilities(next) { support = next; clearDrop(); if (!support.createDirectory) closeDraft(); decorate(); },
    dispose() {
      disposed = true;
      observer.disconnect();
      clearDrop(); closeDraft(); input.remove();
      host.removeEventListener("click", onClick, true);
      host.removeEventListener("keydown", onKeyDown, true);
      host.removeEventListener("dragover", onDragOver);
      host.removeEventListener("dragenter", onDragOver);
      host.removeEventListener("dragleave", onDragLeave);
      host.removeEventListener("drop", onDrop);
    },
  };
}
