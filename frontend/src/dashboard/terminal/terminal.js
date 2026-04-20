import { state } from '../state.js';
import { escapeHtml } from '../util/html.js';
import { getActiveE2EE } from '../e2ee/bridge.js';


export function getTerminalCwd(channelId) {
  if (state.terminalCwdMap.has(channelId)) return state.terminalCwdMap.get(channelId);
  const ch = state.chatChannels.get(channelId);
  const devId = state.channelDeviceMap.get(channelId);
  return ch?.working_directory || (devId ? state.deviceAgentCwd.get(devId) : '') || '';
}

export function shortCwd(cwd) {
  if (!cwd) return '~';
  const parts = cwd.split('/').filter(Boolean);
  return parts.length > 2 ? '…/' + parts.slice(-2).join('/') : cwd;
}

export function renderTerminalCwd() {
  const el = document.getElementById('terminal-cwd');
  const cwd = getTerminalCwd(state.chatCurrentChannel) || '~';
  el.textContent = shortCwd(cwd) + ' $';
  el.title = cwd;
}

export function renderTerminalForChannel(channelId) {
  const output = document.getElementById('terminal-output');
  const promptRow = document.getElementById('terminal-prompt-row');
  // Remove prompt row before clearing, we'll re-append it.
  promptRow?.remove();
  output.innerHTML = '';
  // Reset Up/Down history navigation index for the new channel — otherwise an
  // index from a longer prior channel can land on undefined here.
  state.terminalCmdIndex = state.terminalCmdHistory.length;
  const history = state.terminalHistoryMap.get(channelId) || [];
  for (const entry of history) {
    const block = document.createElement('div');
    block.className = 'terminal-cmd-block';
    const cwdLabel = entry.cwd ? shortCwd(entry.cwd) + ' $' : '$';
    block.innerHTML = `<div class="terminal-cmd-line">${escapeHtml(cwdLabel)} ${escapeHtml(entry.cmd)}</div>`;
    if (entry.output) {
      block.innerHTML += `<div class="terminal-cmd-output">${escapeHtml(entry.output)}</div>`;
    }
    if (entry.exitCode != null && entry.exitCode !== 0) {
      block.innerHTML += `<div class="terminal-cmd-exit error">exit ${entry.exitCode}</div>`;
    }
    output.appendChild(block);
  }
  // Re-append prompt row and show/hide based on running state.
  if (promptRow) {
    output.appendChild(promptRow);
    promptRow.classList.toggle('hidden', state.terminalRunning);
  }
  output.scrollTop = output.scrollHeight;
  renderTerminalCwd();
}

export function terminalExec(command) {
  if (!command || !state.chatCurrentChannel || !getActiveE2EE()?.connected || state.terminalRunning) return;
  state.terminalRunning = true;

  const input = document.getElementById('terminal-input');
  const promptRow = document.getElementById('terminal-prompt-row');
  input.value = '';

  // Add to command history
  state.terminalCmdHistory.push(command);
  state.terminalCmdIndex = state.terminalCmdHistory.length;

  const channelId = state.chatCurrentChannel;
  const cwd = getTerminalCwd(channelId);
  const cwdLabel = cwd ? shortCwd(cwd) + ' $' : '$';

  // Hide prompt row while running.
  promptRow.classList.add('hidden');

  // Create output block (inserted before the prompt row).
  const output = document.getElementById('terminal-output');
  const block = document.createElement('div');
  block.className = 'terminal-cmd-block';
  block.innerHTML = `<div class="terminal-cmd-line">${escapeHtml(cwdLabel)} ${escapeHtml(command)}</div>`;
  const outputDiv = document.createElement('div');
  outputDiv.className = 'terminal-cmd-output';
  block.appendChild(outputDiv);
  output.insertBefore(block, promptRow);
  output.scrollTop = output.scrollHeight;
  const commandId = crypto.randomUUID();
  state.terminalCurrentBlock = { block, outputDiv, channelId, cmd: command, text: '', commandId, hasOutput: false };

  // Track in history
  if (!state.terminalHistoryMap.has(channelId)) state.terminalHistoryMap.set(channelId, []);
  state.terminalHistoryMap.get(channelId).push({ cmd: command, output: '', exitCode: null, cwd });

  getActiveE2EE()?.terminalExec(channelId, command, cwd || undefined, commandId);

  // Show kill button while command is running.
  document.getElementById('terminal-kill-btn')?.classList.remove('hidden');

  // Show braille loading animation if no output within 2 seconds.
  state.terminalLoadingTimer = setTimeout(() => {
    if (state.terminalCurrentBlock && !state.terminalCurrentBlock.hasOutput) {
      const loader = document.createElement('span');
      loader.className = 'terminal-loader';
      state.terminalCurrentBlock.outputDiv.appendChild(loader);
      state.terminalCurrentBlock.loader = loader;
      let frame = 0;
      const frames = ['⠋','⠙','⠹','⠸','⠼','⠴','⠦','⠧','⠇','⠏'];
      state.terminalLoadingInterval = setInterval(() => {
        loader.textContent = frames[frame % frames.length];
        frame++;
      }, 80);
    }
  }, 2000);


  // 30s no-output timeout — if no output arrives, assume command is stuck.
  state.terminalNoOutputTimer = setTimeout(() => {
    if (state.terminalCurrentBlock?.commandId === commandId && !state.terminalCurrentBlock.hasOutput) {
      finishTerminalCommand(
        'Error: Command produced no output for 30 seconds. It may require an interactive terminal.\r\n', 1,
      );
    }
  }, 30000);
}

export function clearTerminalTimers() {
  if (state.terminalLoadingTimer) { clearTimeout(state.terminalLoadingTimer); state.terminalLoadingTimer = null; }
  if (state.terminalLoadingInterval) { clearInterval(state.terminalLoadingInterval); state.terminalLoadingInterval = null; }
  if (state.terminalNoOutputTimer) { clearTimeout(state.terminalNoOutputTimer); state.terminalNoOutputTimer = null; }
  if (state.terminalKillTimer) { clearTimeout(state.terminalKillTimer); state.terminalKillTimer = null; }
  if (state.terminalCurrentBlock?.loader) { state.terminalCurrentBlock.loader.remove(); state.terminalCurrentBlock.loader = null; }
}

export function finishTerminalCommand(errorData, exitCode) {
  clearTerminalTimers();
  if (state.terminalCurrentBlock) {
    if (errorData) {
      const span = document.createElement('span');
      span.className = 'terminal-cmd-exit error';
      span.textContent = errorData;
      state.terminalCurrentBlock.outputDiv.appendChild(span);
    }
    if (exitCode !== 0) {
      const exitDiv = document.createElement('div');
      exitDiv.className = 'terminal-cmd-exit error';
      exitDiv.textContent = `exit ${exitCode}`;
      state.terminalCurrentBlock.block.appendChild(exitDiv);
    }
    state.terminalCurrentBlock = null;
  }
  state.terminalRunning = false;
  document.getElementById('terminal-kill-btn')?.classList.add('hidden');
  const promptRow = document.getElementById('terminal-prompt-row');
  if (promptRow) promptRow.classList.remove('hidden');
  const output = document.getElementById('terminal-output');
  if (output) output.scrollTop = output.scrollHeight;
  document.getElementById('terminal-input')?.focus();
}

export function clearTerminalCompletions() {
  state.terminalCompletions = [];
  state.terminalCompletionIndex = -1;
  state.terminalCompletionBase = '';
  state.terminalCompletionPartial = '';
  document.querySelectorAll('.terminal-completions').forEach(el => el.remove());
}

document.getElementById('terminal-input')?.addEventListener('keydown', (e) => {
  if (e.key === 'Tab') {
    e.preventDefault();
    if (state.terminalCompletionPending || state.terminalRunning) return;

    // If we already have completions, cycle through them
    if (state.terminalCompletions.length > 1) {
      state.terminalCompletionIndex = (state.terminalCompletionIndex + 1) % state.terminalCompletions.length;
      const match = state.terminalCompletions[state.terminalCompletionIndex];
      const suffix = match.endsWith('/') ? '' : ' ';
      e.target.value = state.terminalCompletionBase + match + suffix;
      return;
    }

    const line = e.target.value;
    if (!line) return;
    // Extract the partial word (last whitespace-delimited token)
    const words = line.split(/\s+/);
    const partial = words[words.length - 1] || '';
    const cwd = getTerminalCwd(state.chatCurrentChannel);
    const _conn = getActiveE2EE();
    if (_conn && _conn.connected && state.chatCurrentChannel) {
      state.terminalCompletionPending = true;
      state.terminalCompletionPartial = partial;
      state.terminalCompletionBase = line.slice(0, line.length - partial.length);
      _conn.terminalComplete(state.chatCurrentChannel, partial, line, cwd);
    }
  } else if (e.key === 'Enter' && !e.shiftKey) {
    clearTerminalCompletions();
    e.preventDefault();
    terminalExec(e.target.value.trim());
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    if (state.terminalCmdIndex > 0) {
      state.terminalCmdIndex--;
      e.target.value = state.terminalCmdHistory[state.terminalCmdIndex] || '';
    }
  } else if (e.key === 'ArrowDown') {
    e.preventDefault();
    if (state.terminalCmdIndex < state.terminalCmdHistory.length - 1) {
      state.terminalCmdIndex++;
      e.target.value = state.terminalCmdHistory[state.terminalCmdIndex] || '';
    } else {
      state.terminalCmdIndex = state.terminalCmdHistory.length;
      e.target.value = '';
    }
  } else if (e.key !== 'Shift' && e.key !== 'Control' && e.key !== 'Alt' && e.key !== 'Meta') {
    // Slash while cycling a dir completion — accept it, don't double the slash
    if (e.key === '/' && state.terminalCompletions.length && state.terminalCompletionIndex >= 0) {
      const current = state.terminalCompletions[state.terminalCompletionIndex];
      if (current.endsWith('/')) {
        e.preventDefault();
        clearTerminalCompletions();
        return;
      }
    }
    // Any other key clears completion state
    if (state.terminalCompletions.length) clearTerminalCompletions();
  }
});

document.getElementById('terminal-kill-btn')?.addEventListener('click', () => {
  const _killConn = getActiveE2EE();
  if (_killConn && _killConn.connected && state.chatCurrentChannel) {
    const cmdId = state.terminalCurrentBlock?.commandId || '';
    _killConn.terminalKill(state.chatCurrentChannel, cmdId);
    // If no done frame within 5s, force-reset the terminal locally.
    state.terminalKillTimer = setTimeout(() => {
      if (state.terminalCurrentBlock?.commandId === cmdId) {
        finishTerminalCommand('^C (kill timeout)\r\n', 130);
      }
    }, 5000);
  }
});
