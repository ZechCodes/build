// TerminalView — command input + scrollback output. Mounts into
// #v2-rail-body. One per Channel.
//
// UX notes:
//   - Output is ANSI-SGR aware (see util/ansi.js) so `ls --color`,
//     `git`, `grep` etc. render with colours.
//   - A persistent controls strip hosts ⌃C, ⇥, ⎋ — the first is
//     visible everywhere, the latter two only at ≤768px since a
//     physical keyboard has those keys.
//   - Ctrl+C mirrors real bash: while running it kills the proc;
//     while idle it echoes the typed line with a trailing `^C` and
//     clears the input.

import { bus } from '../../core/bus.js';
import { terminalStore } from '../../domain/terminal-store.js';
import { channelsStore } from '../../domain/channels-store.js';
import { escapeHtml } from '../../util/html.js';
import { ansiToHtml, createAnsiState } from '../../util/ansi.js';

export class TerminalView {
  constructor(channel) {
    this.channel = channel;
    this.root = null;
    this.outputEl = null;
    this.inputEl = null;
    this.promptCwdEl = null;
    this.controlsEl = null;
    this.unsubs = [];
    this._ansi = createAnsiState();
    this._renderedLen = 0;  // chars of output history consumed into the DOM
  }

  activate() {
    this.root = document.getElementById('v2-rail-body');
    if (!this.root) return;
    this._buildShell();
    this._renderHistory({ force: true });
    this._renderPromptRow();
    this.unsubs.push(terminalStore.subscribe(e => {
      if (e.channelId !== this.channel.id) return;
      if (e.kind === 'output') {
        this._renderHistory();
        // appendOutput flips `running=true` in the store; mirror that
        // onto viewState + the root class so ⌃C turns red immediately.
        this._renderPromptRow();
      } else if (e.kind === 'complete') {
        this._renderHistory({ force: true });
        this._renderPromptRow();
      } else if (e.kind === 'clear') {
        this._renderHistory({ force: true });
      } else if (e.kind === 'completions') {
        this._applyCompletions(terminalStore.forChannel(this.channel.id).completions);
      }
    }));
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    this.root = null;
    this.outputEl = null;
    this.inputEl = null;
    this.promptCwdEl = null;
    this.controlsEl = null;
    this._ansi = createAnsiState();
    this._renderedLen = 0;
  }

  _buildShell() {
    this.root.innerHTML = `
      <div class="v2-term">
        <div class="v2-term-output" data-slot="output"></div>
        <div class="v2-term-prompt" data-slot="prompt">
          <span class="v2-term-cwd" data-slot="cwd"></span>
          <span class="v2-term-sigil">$</span>
          <input class="v2-term-input" type="text"
                 autocapitalize="off" autocorrect="off" spellcheck="false"
                 autocomplete="off">
          <div class="v2-term-controls" data-slot="controls">
            <button class="v2-term-ctrl" data-term-cmd="ctrl-c" type="button"
                    title="Interrupt (Ctrl+C)" aria-label="Interrupt (Ctrl+C)">^C</button>
            <button class="v2-term-ctrl v2-term-ctrl-mobile" data-term-cmd="tab" type="button"
                    title="Tab" aria-label="Tab">Tab</button>
            <button class="v2-term-ctrl v2-term-ctrl-mobile" data-term-cmd="esc" type="button"
                    title="Esc" aria-label="Esc">Esc</button>
          </div>
        </div>
      </div>
    `;
    this.outputEl = this.root.querySelector('[data-slot="output"]');
    this.inputEl = this.root.querySelector('.v2-term-input');
    this.promptCwdEl = this.root.querySelector('[data-slot="cwd"]');
    this.controlsEl = this.root.querySelector('[data-slot="controls"]');

    this.inputEl.addEventListener('keydown', this._onKeydown);
    this.controlsEl.addEventListener('click', this._onControlClick);
  }

  _renderHistory({ force = false } = {}) {
    if (!this.outputEl) return;
    const slot = terminalStore.forChannel(this.channel.id);
    const wasNearBottom = this._isNearBottom();

    if (force) {
      // Full rebuild — re-run ANSI state from scratch.
      this.outputEl.innerHTML = '';
      this._ansi = createAnsiState();
      for (const entry of slot.history) this._appendEntry(entry);
    } else {
      // Incremental: the last entry is either a new output chunk or
      // a freshly-appended complete marker. Replace the last node
      // from scratch for both cases — simplest correct behaviour and
      // the DOM cost is bounded by how much the last entry emitted.
      const lastIdx = slot.history.length - 1;
      if (lastIdx < 0) { this.outputEl.innerHTML = ''; return; }
      const last = slot.history[lastIdx];
      // If the last child maps to this entry, rebuild in place;
      // otherwise append a fresh node. The stamped dataset index
      // keeps the 1:1 mapping trivial.
      const lastNode = this.outputEl.lastElementChild;
      const lastNodeIdx = lastNode ? Number(lastNode.dataset.idx) : -1;
      if (lastNode && lastNodeIdx === lastIdx) {
        lastNode.remove();
        // Roll back the ANSI state to just before this entry, then
        // replay. Since we don't cache prior states, the cheapest
        // correct path is a full rebuild when a prior entry also
        // updated. In practice `output` entries arrive with growing
        // text (the store concats), so only the current entry's
        // text is volatile.
        this._ansi = this._ansiBefore(slot.history, lastIdx);
        this._appendEntry(last);
      } else {
        this._appendEntry(last);
      }
    }

    if (wasNearBottom) this.outputEl.scrollTop = this.outputEl.scrollHeight;
  }

  /** Replays ANSI state through every entry strictly before `idx`. */
  _ansiBefore(history, idx) {
    let state = createAnsiState();
    for (let i = 0; i < idx; i++) {
      const e = history[i];
      if (e.type !== 'output' || !e.text) continue;
      state = ansiToHtml(e.text, state).state;
    }
    return state;
  }

  _appendEntry(entry) {
    if (!this.outputEl) return;
    const idx = terminalStore.forChannel(this.channel.id).history.indexOf(entry);
    if (entry.type === 'output') {
      const { html, state } = ansiToHtml(entry.text || '', this._ansi);
      this._ansi = state;
      const div = document.createElement('div');
      div.className = 'v2-term-out';
      div.dataset.idx = String(idx);
      div.innerHTML = html;
      this.outputEl.appendChild(div);
    } else if (entry.type === 'complete') {
      const code = typeof entry.exitCode === 'number' ? entry.exitCode : 0;
      const div = document.createElement('div');
      div.className = `v2-term-complete ${code !== 0 ? 'err' : ''}`;
      div.dataset.idx = String(idx);
      div.innerHTML = `<span class="v2-term-complete-chip">exit ${code}</span>`;
      this.outputEl.appendChild(div);
      // New command starts with a clean SGR slate.
      this._ansi = createAnsiState();
    }
  }

  _renderPromptRow() {
    if (!this.promptCwdEl || !this.inputEl) return;
    const slot = terminalStore.forChannel(this.channel.id);
    const cwd = slot.cwd || this._defaultCwd();
    this.channel.viewState.terminalCwd = cwd || null;
    this.promptCwdEl.textContent = shortCwd(cwd || '~');
    this.channel.viewState.terminalRunning = !!slot.running;
    this.inputEl.disabled = !!slot.running;
    // Decorate ⌃C so the running state is visible even if the user
    // didn't move focus to the input. Class lives on `.v2-term`
    // (root is the rail-body slot which may host other panels).
    this.root?.querySelector('.v2-term')?.classList.toggle('is-running', !!slot.running);
    if (!slot.running) this.inputEl.focus();
  }

  _defaultCwd() {
    const ch = channelsStore.get(this.channel.id);
    return ch?.working_directory || '';
  }

  _onKeydown = (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      this._execCurrent();
      return;
    }
    if (e.key === 'ArrowUp')   { e.preventDefault(); this._historyStep(-1); return; }
    if (e.key === 'ArrowDown') { e.preventDefault(); this._historyStep(1);  return; }
    if (e.key === 'Tab')       { e.preventDefault(); this._requestCompletion(); return; }
    if (e.key === 'Escape')    { e.preventDefault(); this._onEsc();             return; }
    if ((e.ctrlKey || e.metaKey) && (e.key === 'c' || e.key === 'C')) {
      e.preventDefault();
      this._onCtrlC();
    }
  };

  _onControlClick = (e) => {
    const btn = e.target.closest('[data-term-cmd]');
    if (!btn) return;
    const cmd = btn.getAttribute('data-term-cmd');
    if (cmd === 'ctrl-c') this._onCtrlC();
    else if (cmd === 'tab') this._requestCompletion();
    else if (cmd === 'esc') this._onEsc();
    // Keep focus in the input after a tap so the keyboard stays
    // open on mobile.
    this.inputEl?.focus();
  };

  _onCtrlC() {
    if (this.channel.viewState.terminalRunning) {
      bus.emit('intent.terminal_kill', { channelId: this.channel.id });
      return;
    }
    // Idle: mirror bash's behaviour.
    const current = this.inputEl?.value || '';
    if (this.outputEl) {
      const echo = document.createElement('div');
      echo.className = 'v2-term-echo';
      echo.innerHTML =
        `<span class="v2-term-cwd">${escapeHtml(shortCwd(this.channel.viewState.terminalCwd || '~'))}</span>` +
        `<span class="v2-term-sigil">$</span> ` +
        `<span class="v2-term-echo-cmd">${escapeHtml(current)}</span>` +
        `<span class="v2-term-echo-trail">^C</span>`;
      this.outputEl.appendChild(echo);
      this.outputEl.scrollTop = this.outputEl.scrollHeight;
    }
    if (this.inputEl) this.inputEl.value = '';
    this.channel.viewState.terminalCompletions = [];
    this.channel.viewState.terminalCompletionIndex = -1;
  }

  _onEsc() {
    // If completions were offered, clear them; otherwise behave like
    // real bash on Esc (no-op). We also blur on mobile so the native
    // keyboard dismisses — common UX expectation.
    const hadCompletions = !!this.channel.viewState.terminalCompletions?.length;
    this.channel.viewState.terminalCompletions = [];
    this.channel.viewState.terminalCompletionIndex = -1;
    if (!hadCompletions && this.inputEl && window.matchMedia('(max-width: 768px)').matches) {
      this.inputEl.blur();
    }
  }

  _execCurrent() {
    const cmd = this.inputEl.value.trim();
    if (!cmd) return;
    const history = this.channel.viewState.terminalCmdHistory;
    if (history[history.length - 1] !== cmd) history.push(cmd);
    this.channel.viewState.terminalCmdIndex = -1;
    this.channel.viewState.terminalCompletions = [];
    this.channel.viewState.terminalCompletionIndex = -1;

    // Echo command visibly into the scrollback.
    if (this.outputEl) {
      const echo = document.createElement('div');
      echo.className = 'v2-term-echo';
      echo.innerHTML =
        `<span class="v2-term-cwd">${escapeHtml(shortCwd(this.channel.viewState.terminalCwd || '~'))}</span>` +
        `<span class="v2-term-sigil">$</span> ` +
        `<span class="v2-term-echo-cmd">${escapeHtml(cmd)}</span>`;
      this.outputEl.appendChild(echo);
      this.outputEl.scrollTop = this.outputEl.scrollHeight;
    }
    bus.emit('intent.terminal_exec', {
      channelId: this.channel.id,
      command: cmd,
      cwd: this.channel.viewState.terminalCwd || undefined,
    });
    this.inputEl.value = '';
  }

  _historyStep(direction) {
    const history = this.channel.viewState.terminalCmdHistory;
    if (!history.length) return;
    let idx = this.channel.viewState.terminalCmdIndex;
    if (direction < 0) {
      idx = idx === -1 ? history.length - 1 : Math.max(0, idx - 1);
    } else {
      if (idx === -1) return;
      idx = idx + 1 < history.length ? idx + 1 : -1;
    }
    this.channel.viewState.terminalCmdIndex = idx;
    this.inputEl.value = idx === -1 ? '' : history[idx];
    const end = this.inputEl.value.length;
    this.inputEl.setSelectionRange(end, end);
  }

  _requestCompletion() {
    const value = this.inputEl.value;
    const tokens = value.split(/(\s+)/);
    const partial = tokens[tokens.length - 1] || '';
    const base = value.slice(0, value.length - partial.length);
    this.channel.viewState.terminalCompletionBase = base;
    this.channel.viewState.terminalCompletionPartial = partial;
    bus.emit('intent.terminal_complete', {
      channelId: this.channel.id,
      partial,
      line: value,
      cwd: this.channel.viewState.terminalCwd || '',
    });
  }

  _applyCompletions(candidates) {
    if (!candidates?.length) return;
    const base = this.channel.viewState.terminalCompletionBase;
    if (candidates.length === 1) {
      const match = candidates[0];
      const suffix = match.endsWith('/') ? '' : ' ';
      this.inputEl.value = base + match + suffix;
      this.channel.viewState.terminalCompletions = [];
      this.channel.viewState.terminalCompletionIndex = -1;
      return;
    }
    let common = candidates[0];
    for (let i = 1; i < candidates.length; i++) {
      while (common && !candidates[i].startsWith(common)) common = common.slice(0, -1);
    }
    const partial = this.channel.viewState.terminalCompletionPartial;
    if (common.length > partial.length) {
      this.inputEl.value = base + common;
    }
    this.channel.viewState.terminalCompletions = candidates;
    this.channel.viewState.terminalCompletionIndex = -1;
    if (this.outputEl) {
      const list = document.createElement('div');
      list.className = 'v2-term-compls';
      list.textContent = candidates.join('  ');
      this.outputEl.appendChild(list);
      this.outputEl.scrollTop = this.outputEl.scrollHeight;
    }
  }

  _isNearBottom() {
    if (!this.outputEl) return true;
    return this.outputEl.scrollHeight - (this.outputEl.scrollTop + this.outputEl.clientHeight) < 40;
  }
}

function shortCwd(cwd) {
  if (!cwd) return '~';
  const home = cwd.match(/^\/Users\/[^/]+/);
  if (home) return cwd.replace(home[0], '~');
  return cwd;
}
