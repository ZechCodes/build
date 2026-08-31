// Which agent a new one is created on — an account setting, held by the bridge.
//
// Every harness is an agent of its own, and an agent is LOCKED to the one it
// was created on: its conversation lives in that program, so nothing ever moves
// it. That makes the account's answer a starting point rather than a mode — it
// names the harness a new agent is created on where nobody said otherwise,
// which is the option the chat tab's new-agent view leads with and the one the
// bridge falls back to when it has to deliver.
//
// The bridge is the authority (settings.get / settings.set, persisted), so
// every device gets the same answer. The options are the new-agent view's own
// (core/modelPicker.js), so the two controls cannot drift apart.

import { providerOptionsHtml, STARTABLE_PROVIDERS } from "./modelPicker.js";

/** What a bridge that has never been told means. */
export const DEFAULT_HARNESS = STARTABLE_PROVIDERS[0].id;

/** The words a bridge that predates `default_harness` speaks, both ways. Only
 *  the two claude harnesses are sayable in them — a bridge that old has no
 *  other question to ask. */
const HARNESS_OF_CLAUDE_MODE = { headless: "claude_adk", tui: "claude" };
const CLAUDE_MODE_OF_HARNESS = { claude_adk: "headless", claude: "tui" };

/** The harness a settings payload names, read through the old key when that is
 *  all the bridge answers with, and defaulted rather than left empty when it
 *  answers with neither. */
export function defaultHarnessOf(settings) {
  const named = (settings && settings.default_harness) || "";
  if (STARTABLE_PROVIDERS.some((provider) => provider.id === named)) return named;
  return HARNESS_OF_CLAUDE_MODE[(settings && settings.claude_mode) || ""] || DEFAULT_HARNESS;
}

/** The panel, empty. `mountDefaultHarness` fills the select from the bridge —
 *  rendering it pre-filled would show a choice nobody has confirmed is the
 *  account's. */
export function defaultHarnessPanelHtml() {
  return `<div class="panel">
      <h3>🖥️ Default agent</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">The agent a new one is created on when nobody picks: the option a branch's first message leads with, and the one Build creates for itself when it has something to deliver. Every agent keeps the one it was created on, so its conversation stays where it started.</div>
      <div class="field-row" style="display:flex;gap:10px;flex-wrap:wrap">
        <div class="field" style="flex:1;min-width:180px"><label for="defaultharness">Default agent</label>
          <select id="defaultharness" disabled><option>loading…</option></select></div>
      </div>
      <div class="dim" id="harnesssaved" style="font-size:12px;min-height:16px"></div>
      <div class="adderr" id="harnesserr"></div>
    </div>`;
}

/** Wire the panel to the bridge: paint from settings.get, save with
 *  settings.set, and repaint from whatever the bridge answers — the control
 *  shows what the account actually holds, never what was merely attempted. */
export async function mountDefaultHarness(host, { callRpc }) {
  const select = host.querySelector("#defaultharness");
  const saved = host.querySelector("#harnesssaved");
  const error = host.querySelector("#harnesserr");
  if (!select) return;

  const paint = (settings) => {
    select.innerHTML = providerOptionsHtml(STARTABLE_PROVIDERS, defaultHarnessOf(settings));
  };

  /// The save, and the same save in the words an older bridge speaks. A bridge
  /// that does not know `default_harness` refuses it; the two claude harnesses
  /// are still sayable there, and a Codex default honestly is not — that
  /// bridge's own refusal is what the human reads.
  const save = async (harness) => {
    try {
      return await callRpc("settings.set", { default_harness: harness });
    } catch (refusal) {
      const claudeMode = CLAUDE_MODE_OF_HARNESS[harness];
      if (!claudeMode) throw refusal;
      return callRpc("settings.set", { claude_mode: claudeMode });
    }
  };

  try {
    paint(await callRpc("settings.get"));
  } catch (e) {
    error.textContent = e.message;
    return;
  }
  select.disabled = false;

  select.onchange = async () => {
    const chosen = select.value;
    select.disabled = true;
    error.textContent = "";
    saved.textContent = "Saving…";
    try {
      paint(await save(chosen));
      saved.textContent = "Saved. New agents are created on this one; the agents already here keep the one they were created on.";
    } catch (e) {
      error.textContent = e.message;
      saved.textContent = "";
      try {
        paint(await callRpc("settings.get"));
      } catch {
        /* the refusal is already on screen; a second failure adds nothing */
      }
    }
    select.disabled = false;
  };
}
