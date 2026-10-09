import { esc } from "./text.js";
import { NEW_ENTRIES, newEntryError, newEntryKinds, uploadNameValid } from "./fileUploadsModel.js";
import { parentPath } from "./fileTreeModel.js";
import { fieldTraits } from "./fieldTraits.js";
import { notifyError } from "./notify.js";
import { ICON_UPLOAD, ICON_FOLDER_PLUS } from "./icons.js";
import { mountLongPress } from "./longPress.js";
import { closeSplitMenusWithin, menuButtonMarkup, mountSplitMenu } from "./splitButton.js";
import "../styles/fileUploadActions.css";

const actionButton = (action, title, mark) => `<button type="button" class="iconbtn" data-upload-action="${action}" aria-label="${title}" title="${title}">${mark}</button>`;
// A bridge that can create files makes New a menu; one that can only create
// folders keeps the single New folder button it has always had.
const newHtml = (support) => {
  const kinds = newEntryKinds(support);
  if (!support.createFile) return kinds.length ? actionButton("folder", NEW_ENTRIES.folder.label, ICON_FOLDER_PLUS) : "";
  const items = kinds.map((kind) => ({ id: kind, label: NEW_ENTRIES[kind].label }));
  return `<span class="fupload-new">${menuButtonMarkup("", items, { title: "New file or folder", iconHtml: ICON_FOLDER_PLUS })}</span>`;
};
const actionsHtml = (support) => `<span class="fupload-directory-actions">${support.uploads ? actionButton("upload", "Upload files", ICON_UPLOAD) : ""}${newHtml(support)}</span>`;
const offersActions = (support) => support.uploads || newEntryKinds(support).length > 0;
const fileDrag = (event) => Array.from(event.dataTransfer?.types || []).includes("Files");
const rootIdOf = (row) => row.closest("[data-root]")?.dataset.root ?? row.dataset.rootHead;
const directoryRows = (host) => [...host.querySelectorAll('[data-kind="dir"], [data-root-head], [data-upload-root]')];
const interactive = (event) => event.target.closest?.("[data-upload-action], .fupload-new, .fupload-folder, .fupload-input");
// The New menu answers its own clicks and keys (core/splitButton.js); the
// host's capture handlers must let them reach it.
const inNewMenu = (event) => event.target.closest?.(".fupload-new");
const keepFromTree = (event) => event.stopPropagation();
const actionRow = (event) => {
  const row = event.target.closest?.(".frow");
  return row?.querySelector(".fupload-directory-actions") ? row : null;
};
const destination = (root, parent) => [root.label, parent].filter(Boolean).join(" / ") || "/";

/** Directory affordances decorate existing rows; drag feedback only changes a
 * class and an overlay attribute, preserving the tree's elements and geometry. */
export function mountFilesUploadActions(host, { roots, capabilities, uploads, callRpc, onCreated, onFinished }) {
  let support = capabilities;
  let disposed = false;
  let dropRow = null;
  let selectedTarget = null;
  let draft = null;
  const decoratedMarkup = new WeakMap();
  const input = document.createElement("input");
  input.className = "fupload-input";
  input.type = "file";
  input.multiple = true;
  input.hidden = true;
  host.append(input);

  const rootOf = (row) => roots.find((root) => String(root.id) === rootIdOf(row)) || (roots.length === 1 ? roots[0] : null);
  const parentOf = (row) => row.dataset.kind === "dir" ? row.dataset.path : "";
  const targetOfRow = (row) => ({ root: rootOf(row), parent: parentOf(row), row });
  const rootHead = (root) => directoryRows(host).find((row) => rootOf(row) === root && !row.dataset.path);
  const parentRow = (root, path) => directoryRows(host).find((row) => rootOf(row) === root && row.dataset.path === path) || rootHead(root);

  const targetOf = (event) => {
    const row = event.target.closest?.(".frow");
    if (row?.dataset.kind === "dir") return targetOfRow(row);
    const root = row ? rootOf(row) : rootOf(event.target.closest?.("[data-root]") || host);
    if (!root) return null;
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

  const uploadTarget = ({ root, parent }) => ({ scope: { ...root.scope }, rootId: root.id, parent, destination: destination(root, parent), callRpc, onFinished });
  const createdDirectory = ({ scope, rootId, parent, path }) => {
    const root = roots.find((candidate) => candidate.id === rootId);
    if (root) onCreated({ ...root, scope }, parent, path);
  };
  const onDragOver = (event) => {
    if (!fileDrag(event)) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
    const target = targetOf(event);
    if (support.uploads && target?.root) markDrop(target);
    else clearDrop();
  };
  const onDragLeave = (event) => {
    if (!host.contains(event.relatedTarget)) clearDrop();
  };
  const onDrop = (event) => {
    if (!fileDrag(event)) return;
    event.preventDefault();
    const target = targetOf(event);
    clearDrop();
    if (!target?.root) return;
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
  const createEntry = async () => {
    const current = draft;
    const entry = NEW_ENTRIES[current.kind];
    const name = current.field.value;
    if (!uploadNameValid(name)) return showDraftError(entry.invalid);
    current.field.disabled = true;
    current.error.textContent = "";
    const { root, parent } = current.target;
    const scope = { ...root.scope };
    try {
      const result = await callRpc(entry.method, { ...scope, parent, name });
      if (!disposed && draft === current) closeDraft();
      onCreated({ ...root, scope }, parent, result.path, current.kind);
    } catch (error) {
      if (!disposed && draft === current) showDraftError(newEntryError(current.kind, error));
    }
  };
  const newEntry = (target, kind) => {
    closeDraft();
    const entry = NEW_ENTRIES[kind];
    const element = document.createElement("div");
    element.className = "fupload-folder";
    element.style.setProperty("--depth", Number(target.row.style.getPropertyValue("--depth") || 0) + 1);
    element.innerHTML = `<input class="mini" type="text" aria-label="${entry.field}" placeholder="${entry.placeholder}" ${fieldTraits("identifier", "done")}><span class="fupload-folder-error" role="alert"></span>`;
    target.row.after(element);
    draft = { kind, target, element, field: element.querySelector("input"), error: element.querySelector("span") };
    draft.field.focus();
  };
  const mountNewMenu = (row) => {
    const container = row.querySelector(".fupload-new");
    if (!container) return;
    container.addEventListener("click", keepFromTree);
    container.addEventListener("keydown", keepFromTree);
    mountSplitMenu(container, { onChoose: (kind) => { if (NEW_ENTRIES[kind]) newEntry(targetOfRow(row), kind); } });
  };
  const onClick = (event) => {
    if (!interactive(event) || inNewMenu(event)) return;
    event.stopPropagation();
    const button = event.target.closest("[data-upload-action]");
    if (!button) return;
    const target = targetOfRow(button.closest(".frow"));
    if (button.dataset.uploadAction === "folder") return newEntry(target, "folder");
    selectedTarget = target;
    input.value = "";
    input.click();
  };
  const onKeyDown = (event) => {
    if (!interactive(event) || inNewMenu(event)) return;
    event.stopPropagation();
    if (!event.target.closest(".fupload-folder")) return;
    if (event.key === "Escape") { event.preventDefault(); closeDraft(); }
    if (event.key === "Enter") { event.preventDefault(); if (!draft.field.disabled) void createEntry(); }
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
    // SVG serialization expands self-closing paths; comparing outerHTML to
    // the raw icon would continually replace buttons in the mutation observer.
    if (held && decoratedMarkup.get(row) === markup) return;
    if (held) closeSplitMenusWithin(held);
    held?.remove();
    if (offersActions(support)) {
      row.insertAdjacentHTML("beforeend", markup);
      mountNewMenu(row);
    }
    decoratedMarkup.set(row, markup);
  };
  const restoreDraft = () => {
    if (!draft || draft.element.isConnected) return;
    draft.target.row = parentRow(draft.target.root, draft.target.parent);
    draft.target.row?.after(draft.element);
  };
  const decorate = () => {
    if (disposed) return;
    if (offersActions(support)) ensureRoot();
    const markup = actionsHtml(support);
    directoryRows(host).forEach((row) => decorateRow(row, markup));
    // A cache listing can repaint while a name is being typed. Keep that field
    // at its level without replacing it or losing the draft.
    restoreDraft();
  };
  const observer = new MutationObserver(decorate);
  observer.observe(host, { childList: true, subtree: true });
  decorate();
  const longPress = mountLongPress(host, {
    targetOf: actionRow,
    canStart: (event) => !interactive(event),
    onReveal: (row) => row.classList.add("fupload-revealed"),
    onDismiss: (row) => row.classList.remove("fupload-revealed"),
  });
  host.addEventListener("click", onClick, true);
  host.addEventListener("keydown", onKeyDown, true);
  host.addEventListener("dragover", onDragOver);
  host.addEventListener("dragenter", onDragOver);
  host.addEventListener("dragleave", onDragLeave);
  host.addEventListener("drop", onDrop);
  input.addEventListener("change", onChange);
  return {
    setCapabilities(next) { support = next; longPress.clear(); clearDrop(); if (draft && !newEntryKinds(support).includes(draft.kind)) closeDraft(); decorate(); },
    dispose() {
      disposed = true;
      observer.disconnect();
      closeSplitMenusWithin(host);
      longPress.dispose();
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
