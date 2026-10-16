import { App } from "../appState.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { readUiRecord, writeUiRecordIfUnwritten } from "./localUiStore.js";

/** Branch drafts belong to a capture on one device and one destination
 * project. A row can leave the DOM without ending any of those identities. */
export function createInboxCaptureDrafts({ entryOf, onChange }) {
  const accountEpoch = App.accountEpoch;
  const drafts = new Map();
  const fields = new WeakMap();
  const active = () => accountEpoch === App.accountEpoch;

  function draftFor(deviceId, captureId, projectId) {
    const key = JSON.stringify([deviceId || "", captureId, projectId]);
    if (!drafts.has(key)) {
      const draft = { branch: "", address: uiAddress({ deviceId: deviceId || "", entityId: captureId,
        view: "inbox-capture-branch", kind: "draft", sub: projectId }) };
      draft.record = watchUiState(draft.address, (saved) => {
        const branch = typeof saved?.branch === "string" ? saved.branch : "";
        if (branch === draft.branch) return;
        draft.branch = branch;
        onChange();
      }, { debounceMs: 180, active });
      drafts.set(key, draft);
    }
    return drafts.get(key);
  }

  function draftOf(field) {
    const row = field.closest(".capture-entry");
    const entry = entryOf(row?.dataset.key);
    return entry ? draftFor(entry.deviceId, entry.captureId, field.dataset.rerouteBranch) : null;
  }

  function remember(field) {
    const held = fields.get(field);
    if (!held || field.value === held.value) return;
    held.draft.branch = field.value;
    held.value = field.value;
    held.draft.record.schedule({ branch: field.value });
  }

  return {
    remember(list) {
      list?.querySelectorAll("[data-reroute-branch]").forEach(remember);
    },
    input(event) {
      const field = event.target.closest("[data-reroute-branch]");
      if (field) remember(field);
    },
    restore(list) {
      for (const field of list.querySelectorAll("[data-reroute-branch]")) {
        const draft = draftOf(field);
        if (!draft) continue;
        if (field.value !== draft.branch) field.value = draft.branch;
        fields.set(field, { draft, value: field.value });
      }
    },
    async submission(entry, destination) {
      if (destination?.kind !== "branch") return null;
      const key = JSON.stringify([entry.deviceId || "", entry.captureId, destination.projectId]);
      const draft = drafts.get(key);
      if (!draft) return null;
      await draft.record.flush();
      const record = await readUiRecord(draft.address);
      return record?.value?.branch?.trim() === (destination.branch || "") ? { address: draft.address, record } : null;
    },
    async clear(submission) {
      if (submission && active()) await writeUiRecordIfUnwritten(submission.address, submission.record, { branch: "" });
    },
    dispose() {
      for (const draft of drafts.values()) draft.record.dispose();
      drafts.clear();
    },
  };
}
