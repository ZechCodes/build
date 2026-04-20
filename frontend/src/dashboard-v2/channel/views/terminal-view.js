// TerminalView — command input + scrollback output.
// Mounts into #v2-tab-terminal. One per Channel.

import { bus } from '../../core/bus.js';
import { terminalStore } from '../../domain/terminal-store.js';
import { channelsStore } from '../../domain/channels-store.js';
import { escapeHtml } from '../../util/html.js';

export class TerminalView {
  constructor(channel) {
    this.channel = channel;
    this.root = null;
    this.outputEl = null;
    this.inputEl = null;
    this.promptCwdEl = null;
    this.killBtn = null;
    this.unsubs = [];
  }

  activate() {
    this.root = document.getElementById('v2-rail-body');
    if (!this.root) return;
    this._buildShell();
    this._renderHistory();
    this._renderPromptRow();
    this.unsubs.push(terminalStore.subscribe(e => {
      if (e.channelId !== this.channel.id) return;
      if (e.kind === 'output' || e.kind === 'complete' || e.kind === 'clear') {
        this._renderHistory();
        if (e.kind === 'complete') this._renderPromptRow();
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
    this.killBtn = null;
  }

  _buildShell() {
    this.root.innerHTML = `
      <div class="v2-term">
        <div class="v2-term-output" data-slot="output"></div>
        <div class="v2-term-prompt" data-slot="prompt">
          <span class="v2-term-cwd" data-slot="cwd"></span>
          <span class="v2-term-sigil">$</span>
          <input class="v2-term-input" type="text" autocapitalize="off" autocorrect="off" spellcheck="false">
          <button class="v2-term-kill" type="button" hidden>Kill</button>
        </div>
      </div>
    `;
    this.outputEl = this.root.querySelector('[data-slot="output"]');
    this.inputEl = this.root.querySelector('.v2-term-input');
    this.promptCwdEl = this.root.querySelector('[data-slot="cwd"]');
    this.killBtn = this.root.querySelector('.v2-term-kill');

    this.inputEl.addEventListener('keydown', this._onKeydown);
    this.killBtn.addEventListener('click', () => {
      bus.emit('intent.terminal_kill', { channelId: this.channel.id });
    });
  }

  _renderHistory() {
    if (!this.outputEl) return;
    const slot = terminalStore.forChannel(this.channel.id);
    if (!slot.history.length) {
      this.outputEl.innerHTML = '';
      return;
    }
    const wasNearBottom = this._isNearBottom();
    const parts = [];
    for (const entry of slot.history) {
      if (entry.type === 'output') {
        parts.push(`<div class="v2-term-out">${escapeHtml(entry.text || '')}</div>`);
      } else if (entry.type === 'complete') {
        const code = typeof entry.exitCode === 'number' ? entry.exitCode : 0;
        parts.push(`<div class="v2-term-complete ${code !== 0 ? 'err' : ''}">exit ${code}</div>`);
      }
    }
    this.outputEl.innerHTML = parts.join('');
    if (wasNearBottom) this.outputEl.scrollTop = this.outputEl.scrollHeight;
  }

  _renderPromptRow() {
    if (!this.promptCwdEl || !this.inputEl || !this.killBtn) return;
    const slot = terminalStore.forChannel(this.channel.id);
    const cwd = slot.cwd || this._defaultCwd();
    this.channel.viewState.terminalCwd = cwd || null;
    this.promptCwdEl.textContent = shortCwd(cwd || '~');
    this.channel.viewState.terminalRunning = !!slot.running;
    this.inputEl.disabled = !!slot.running;
    this.killBtn.hidden = !slot.running;
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
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      this._historyStep(-1);
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      this._historyStep(1);
      return;
    }
    if (e.key === 'Tab') {
      e.preventDefault();
      this._requestCompletion();
      return;
    }
    if ((e.ctrlKey || e.metaKey) && (e.key === 'c' || e.key === 'C')) {
      if (this.channel.viewState.terminalRunning) {
        e.preventDefault();
        bus.emit('intent.terminal_kill', { channelId: this.channel.id });
      }
    }
  };

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
      echo.innerHTML = `<span class="v2-term-cwd">${escapeHtml(shortCwd(this.channel.viewState.terminalCwd || '~'))}</span><span class="v2-term-sigil">$</span> ${escapeHtml(cmd)}`;
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
    // Find longest common prefix.
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
    // Show candidates list in output area for this tick.
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
