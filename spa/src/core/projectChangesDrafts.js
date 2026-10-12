// Old source locations keep their own drafts. Expose their text for explicit
// recovery without attaching it to a replacement source's commit action.
import { App } from "../appState.js";
import { subscribeUiRecords, uiDraftRecords } from "./localUiStore.js";
import { applyFieldTraits } from "./fieldTraits.js";

const validSourceAddress = ([owner, sourceId, path, extra], projectId) => owner === projectId
  && extra === undefined && typeof sourceId === "string" && typeof path === "string" && sourceId && path;

function sourceDraft(entry, projectId) {
  const { address, record } = entry;
  if (address.sub !== address.entityId || !address.entityId.startsWith("project:")) return null;
  try {
    const parts = JSON.parse(address.entityId.slice("project:".length));
    if (!validSourceAddress(parts, projectId)) return null;
    if (typeof record.value?.text !== "string" || !record.value.text) return null;
    const [, sourceId, path] = parts;
    return { sourceId, path, text: record.value.text };
  } catch { return null; }
}

export function recoverableSourceDrafts(entries, project) {
  return entries.map((entry) => sourceDraft(entry, project.id || project.project_id)).filter((draft) => draft
    && !project.sources?.some((source) => source.id === draft.sourceId && source.path === draft.path));
}

function draftRow(draft, project) {
  const row = document.createElement("label");
  row.dataset.recoveredDraft = draft.sourceId;
  const source = project.sources?.find((item) => item.id === draft.sourceId);
  const caption = document.createElement("span");
  caption.textContent = `${source?.name || draft.sourceId} · ${draft.path} · source ${source ? "moved" : "removed"}`;
  const field = document.createElement("textarea");
  applyFieldTraits(field, "prose");
  field.className = "csinput";
  field.readOnly = true;
  field.value = draft.text;
  row.append(caption, field);
  return row;
}

function paintDrafts(host, drafts, project) {
  host.replaceChildren();
  host.hidden = !drafts.length;
  if (!drafts.length) return;
  const title = document.createElement("h3");
  title.textContent = "Saved commit drafts";
  const help = document.createElement("p");
  help.textContent = "Copy the text from a previous source location to recover its draft.";
  host.append(title, help, ...drafts.map((draft) => draftRow(draft, project)));
}

export function mountProjectChangesDrafts(host, { deviceId, project }) {
  const accountEpoch = App.accountEpoch;
  let disposed = false;
  let revision = 0;
  let painted = "";
  const read = async () => {
    const current = ++revision;
    const entries = await uiDraftRecords(deviceId);
    if (disposed || current !== revision || accountEpoch !== App.accountEpoch) return;
    const drafts = recoverableSourceDrafts(entries, project);
    const next = JSON.stringify([drafts, project.sources]);
    if (next === painted) return;
    painted = next;
    paintDrafts(host, drafts, project);
  };
  const unwatch = subscribeUiRecords({ deviceId, kind: "ui-draft" }, () => { void read(); });
  void read();
  return {
    update(next) { project = next; void read(); },
    dispose() { disposed = true; unwatch(); },
  };
}
