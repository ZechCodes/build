// Scope settings are cache observations; edits stay local until the adapter
// accepts them. Every repaint updates standing nodes, preserving draft focus.
import "../styles/scoped-compaction.css";
import { esc } from "./text.js";
import { compactionTokenLabel, inheritedCompactionLabel, positiveCompactionTokens, COMPACTION_TIMING, COMPACTION_SAVED } from "./compactionPresentation.js";
import { compactionSupportAddress } from "./compactionSupport.js";
import { captureCachedRecord, mergeCachedRecordsTogether, recordWriteOf } from "./localCache.js";
import { pushFence, pushedSince } from "./pushFence.js";
import { deviceSettingsAddress, projectSettingsAddress, workspaceSettingsAddress, watchSettingsRecord } from "./settingsRecords.js";

const harnesses = [
  ["claude", "Claude Code TUI"], ["claude_adk", "Claude Code"],
  ["codex", "Codex TUI"], ["codex_app_server", "Codex app-server"], ["pi", "Pi"],
];
const scopes = {
  device: { heading: "Compaction defaults on this device", address: deviceSettingsAddress, method: "settings.set" },
  project: { heading: "Compaction defaults for this project", address: projectSettingsAddress, method: "project.set_compaction" },
  workspace: { heading: "Compaction defaults for this workspace", address: workspaceSettingsAddress, method: "workspace.set_compaction" },
};
const OLD_BRIDGE = "Update the bridge to set compaction defaults by harness.";
const INVALID_TOKENS = "Enter a positive whole number of tokens up to 9,007,199,254,740,991.";
const inheritedLabel = (policy) => inheritedCompactionLabel(policy).replace("Use inherited setting", "Inherit");
const thresholdText = (value) => positiveCompactionTokens(value) ? String(value) : value?.tokens_decimal || "";

function rowHtml([provider, label], prefix) {
  const id = `${prefix}-${provider}`;
  return `<div class="scoped-compaction-row" data-compaction-provider="${provider}" data-compaction-harness="${provider}">
    <h4>${label}</h4>
    <div class="scoped-compaction-controls">
      <label for="${esc(id)}">Compact at</label>
      <input id="${esc(id)}" data-compaction-tokens type="text" inputmode="numeric" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false" enterkeyhint="done" aria-label="${label}: Compact at" aria-describedby="${esc(id)}-effective ${esc(id)}-status">
      <span>tokens</span>
      <button type="button" class="btn mini" data-compaction-save data-compaction-action="save" disabled>Save</button>
    </div>
    <p id="${esc(id)}-effective" class="sub" data-compaction-effective></p>
    <button type="button" class="btn mini scoped-compaction-inherit" data-compaction-inherit data-compaction-action="inherit" disabled>Inherit</button>
    <p id="${esc(id)}-status" class="sub scoped-compaction-status" role="status" aria-live="polite"></p>
  </div>`;
}

export function scopedCompactionEditorHtml({ scope, prefix = `compaction-${scope}` }) {
  return `<section class="scoped-compaction" data-compaction-editor="${esc(prefix)}" aria-labelledby="${esc(prefix)}-heading">
    <h3 id="${esc(prefix)}-heading">${scopes[scope].heading}</h3>
    <p class="sub">${COMPACTION_TIMING}</p>
    <p class="sub" data-compaction-support role="status" hidden></p>
    ${harnesses.map((harness) => rowHtml(harness, prefix)).join("")}
  </section>`;
}

function rowControls(section, [provider]) {
  const root = section.querySelector(`[data-compaction-provider="${provider}"]`);
  return { provider, root, input: root.querySelector("input"), save: root.querySelector("[data-compaction-save]"),
    inherit: root.querySelector("[data-compaction-inherit]"), effective: root.querySelector("[data-compaction-effective]"),
    status: root.querySelector('[role="status"]'), revision: 0, dirty: false };
}

function observedRow(value, provider) {
  return { threshold: thresholdText(value?.compaction_thresholds?.[provider]),
    inherited: value?.compaction?.inherited?.[provider], effective: value?.compaction?.effective?.[provider] };
}

function updateInputValue(input, value) {
  if (input.value === value) return;
  const focused = input.ownerDocument.activeElement === input;
  const selection = [input.selectionStart, input.selectionEnd, input.selectionDirection];
  input.value = value;
  if (focused) input.setSelectionRange(...selection);
}

function paintRow(row, state) {
  const { value, supported, pending } = state;
  const { threshold, inherited, effective } = observedRow(value, row.provider);
  if (!row.dirty && !pending) updateInputValue(row.input, threshold);
  row.inherit.textContent = inheritedLabel(inherited);
  row.effective.textContent = effective ? `Effective: ${compactionTokenLabel(effective)}` : "";
  row.save.disabled = !supported || !value || Boolean(pending);
  row.inherit.disabled = row.save.disabled;
  row.input.readOnly = !supported;
}

function saveErrorMessage(error) {
  const message = typeof error?.message === "string" ? error.message.trim() : "";
  if (error?.code === "unknown_method") return OLD_BRIDGE;
  return `Build could not save this compaction default. ${message || "Try again."}`;
}

const canSave = (state) => state.active && state.supported && state.value && !state.pending;

const parameters = (scope, ownerId, provider, tokens) => ({
  ...(scope === "device" ? {} : { [`${scope}_id`]: ownerId }), compaction_thresholds: { [provider]: tokens },
});
const validAnswer = (scope, ownerId, answer) => answer?.compaction && answer?.compaction_thresholds
  && (scope === "device" || answer[`${scope}_id`] === ownerId);

async function acceptAnswer(address, before, fence, answer, current) {
  return mergeCachedRecordsTogether([address], ([held]) => current() && !pushedSince(address, fence)
    && recordWriteOf(held) === recordWriteOf(before) ? [answer] : null);
}

function parseInput(row) {
  const text = row.input.value.trim();
  const tokens = /^[0-9]+$/.test(text) ? Number(text) : null;
  if (positiveCompactionTokens(tokens)) return tokens;
  row.status.textContent = INVALID_TOKENS;
  row.input.setAttribute("aria-invalid", "true");
  return undefined;
}

/** Mount only subscribes to the cache. Existing scope readers and compaction
 * invalidations own pulls, so opening an editor never creates another one. */
export function mountScopedCompactionEditor(root, { scope, ownerId, deviceId, callRpc, prefix = `compaction-${scope}` }) {
  const section = [...root.querySelectorAll("[data-compaction-editor]")].find((node) => node.dataset.compactionEditor === prefix);
  const address = scopes[scope].address(deviceId, ownerId);
  const state = { active: true, value: null, supported: false, pending: null };
  const rows = harnesses.map((harness) => rowControls(section, harness));
  const paint = () => rows.forEach((row) => paintRow(row, state));
  const record = watchSettingsRecord(address, (value) => { state.value = value; paint(); }, { owner: section });
  const support = watchSettingsRecord(compactionSupportAddress(deviceId), (value) => {
    state.supported = value?.harnessThresholds === true;
    const message = section.querySelector("[data-compaction-support]");
    message.textContent = OLD_BRIDGE;
    message.hidden = !value || state.supported;
    paint();
  }, { owner: section });

  async function saveRow(row, tokens) {
    if (!canSave(state)) return;
    const revision = row.revision;
    state.pending = row;
    row.status.textContent = "Saving…";
    row.input.removeAttribute("aria-invalid");
    paint();
    try {
      const fence = pushFence();
      const before = await captureCachedRecord(address);
      if (!state.active) return;
      const answer = await callRpc(scopes[scope].method, parameters(scope, ownerId, row.provider, tokens));
      if (!validAnswer(scope, ownerId, answer)) throw new Error("Invalid compaction settings answer");
      await acceptAnswer(address, before, fence, answer, () => state.active && state.supported);
      if (!state.active) return;
      if (row.revision === revision) row.dirty = false;
      row.status.textContent = COMPACTION_SAVED;
    } catch (error) {
      if (state.active) row.status.textContent = saveErrorMessage(error);
    } finally {
      state.pending = null;
      if (state.active) { await record.read(); paint(); }
    }
  }

  function onInput(event) {
    const row = rows.find((row) => row.input === event.target);
    if (!row) return;
    row.dirty = true;
    row.revision += 1;
    row.status.textContent = "";
    row.input.removeAttribute("aria-invalid");
  }
  function onClick(event) {
    const button = event.target.closest("[data-compaction-action]");
    if (!button || button.disabled) return;
    const row = rows.find((row) => row.root.contains(button));
    const tokens = button.dataset.compactionAction === "inherit" ? null : parseInput(row);
    if (tokens !== undefined) void saveRow(row, tokens);
  }
  function onKeydown(event) {
    if (event.key !== "Enter" || !event.target.matches("[data-compaction-tokens]")) return;
    event.preventDefault();
    const row = rows.find((row) => row.input === event.target);
    row.save.click();
  }
  section.addEventListener("input", onInput);
  section.addEventListener("click", onClick);
  section.addEventListener("keydown", onKeydown);
  // The same section may be transplanted during a synchronous sheet repaint.
  // Only a removal that is still disconnected at observation retires this owner.
  const observer = new MutationObserver(() => { if (!section.isConnected) dispose(); });
  observer.observe(section.ownerDocument.documentElement, { childList: true, subtree: true });
  function dispose() {
    state.active = false;
    record.dispose(); support.dispose(); observer.disconnect();
    section.removeEventListener("input", onInput);
    section.removeEventListener("click", onClick);
    section.removeEventListener("keydown", onKeydown);
  }
  return { read: () => Promise.all([record.read(), support.read()]), dispose };
}
