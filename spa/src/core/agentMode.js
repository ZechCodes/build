// Which program each agent opens — an account setting, held by the bridge.
//
// Claude Code comes in two programs: one that runs as a conversation, and one
// that runs as a terminal program. They are the same agent, the same account and
// the same transcripts, so a person picking between them at every start would be
// answering the same question forever and ending up with two agents side by side
// that nothing but a carrier name tells apart. The choice is made once here, and
// every start of "Claude Code" opens whichever program this names.
//
// The bridge is the authority (settings.get / settings.set, persisted), so every
// device gets the same answer. Codex is wired the same way and locked: there is
// only one Codex program today.

import { esc } from "./text.js";

/** The programs Claude Code can be, in the order a person meets them. The ids
 *  are the bridge's wire words; the labels are the only words a person reads. */
export const CLAUDE_MODES = [
  { id: "headless", label: "Claude Code" },
  { id: "tui", label: "Claude Code TUI" },
];

/** Codex's one program. The field exists so the Account page has one idiom, and
 *  the day there is a second the lock comes off. */
export const CODEX_MODES = [{ id: "tui", label: "Codex TUI" }];

/** What a bridge that has never been told means. */
export const DEFAULT_CLAUDE_MODE = "headless";

const optionsHtml = (modes, selected) =>
  modes
    .map((mode) => `<option value="${esc(mode.id)}"${mode.id === selected ? " selected" : ""}>${esc(mode.label)}</option>`)
    .join("");

/** The mode a settings payload names, or the default — an older bridge does not
 *  answer this question at all, and silence is the default, not an empty
 *  control. */
export function claudeModeOf(settings) {
  const named = settings && typeof settings.claude_mode === "string" ? settings.claude_mode : "";
  return CLAUDE_MODES.some((mode) => mode.id === named) ? named : DEFAULT_CLAUDE_MODE;
}

/** Same for Codex, which has one answer. */
export function codexModeOf(settings) {
  const named = settings && typeof settings.codex_mode === "string" ? settings.codex_mode : "";
  return CODEX_MODES.some((mode) => mode.id === named) ? named : CODEX_MODES[0].id;
}

/** The panel, empty. `mountAgentMode` fills the two selects from the bridge —
 *  rendering them pre-filled would show a choice nobody has confirmed is the
 *  account's. */
export function agentModePanelHtml() {
  return `<div class="panel">
      <h3>🖥️ How agents run</h3>
      <div class="dim" style="font-size:13px;margin-bottom:10px">Claude Code runs as a conversation. Its TUI is the same agent, same account and same transcripts, running as a terminal program instead. One choice for the whole account, so every worktree runs the same one.</div>
      <div class="field-row" style="display:flex;gap:10px;flex-wrap:wrap">
        <div class="field" style="flex:1;min-width:180px"><label>Claude Code mode</label><select id="claudemode" disabled><option>loading…</option></select></div>
        <div class="field" style="flex:1;min-width:180px"><label>Codex mode</label><select id="codexmode" disabled><option>loading…</option></select>
          <div class="dim" id="codexmodenote" style="font-size:12px">Codex comes in one program today.</div></div>
      </div>
      <div class="dim" id="modesaved" style="font-size:12px;min-height:16px"></div>
      <div class="adderr" id="modeerr"></div>
    </div>`;
}

/** Wire the panel to the bridge: paint from settings.get, save with
 *  settings.set, and repaint from whatever the bridge answers — the control
 *  shows what the account actually holds, never what was merely attempted. */
export async function mountAgentMode(host, { callRpc }) {
  const claudeSelect = host.querySelector("#claudemode");
  const codexSelect = host.querySelector("#codexmode");
  const saved = host.querySelector("#modesaved");
  const error = host.querySelector("#modeerr");
  if (!claudeSelect || !codexSelect) return;

  const paint = (settings) => {
    claudeSelect.innerHTML = optionsHtml(CLAUDE_MODES, claudeModeOf(settings));
    codexSelect.innerHTML = optionsHtml(CODEX_MODES, codexModeOf(settings));
  };

  try {
    paint(await callRpc("settings.get"));
  } catch (e) {
    error.textContent = e.message;
    return;
  }
  claudeSelect.disabled = false;

  claudeSelect.onchange = async () => {
    const chosen = claudeSelect.value;
    claudeSelect.disabled = true;
    error.textContent = "";
    saved.textContent = "Saving…";
    try {
      paint(await callRpc("settings.set", { claude_mode: chosen }));
      saved.textContent = "Saved. New agents start this way; the ones already running keep the program they opened with.";
    } catch (e) {
      error.textContent = e.message;
      saved.textContent = "";
      try {
        paint(await callRpc("settings.get"));
      } catch {
        /* the refusal is already on screen; a second failure adds nothing */
      }
    }
    claudeSelect.disabled = false;
  };
}
