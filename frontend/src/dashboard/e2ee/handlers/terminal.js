import { state } from '../../state.js';
import { escapeHtml } from '../../util/html.js';
import { renderTerminalCwd, renderTerminalForChannel, finishTerminalCommand, clearTerminalTimers, clearTerminalCompletions } from '../../terminal/terminal.js';

export function bindTerminalHandlers(instance, deviceId) {
  instance.addEventListener('terminal_output', (evt) => {
    const { channel_id, data, done, exit_code, cwd } = evt.detail;
    if (!channel_id) return;

    const sentinel = '__BUILD_CWD__';

    if (!done && data) {
      // First output received — clear loading animation and reset no-output timer.
      if (state.terminalCurrentBlock && !state.terminalCurrentBlock.hasOutput) {
        state.terminalCurrentBlock.hasOutput = true;
        clearTerminalTimers();
      }

      // Filter out sentinel line from streaming output.
      let text = data;
      if (text.includes(sentinel)) {
        text = text.split('\n').filter(l => !l.startsWith(sentinel)).join('\n');
        if (!text) return;
      }

      // Append to current streaming block if matching channel.
      if (state.terminalCurrentBlock && state.terminalCurrentBlock.channelId === channel_id) {
        state.terminalCurrentBlock.text += text;
        const span = document.createElement('span');
        span.textContent = text;
        state.terminalCurrentBlock.outputDiv.appendChild(span);
        const output = document.getElementById('terminal-output');
        output.scrollTop = output.scrollHeight;
      }
      // Update stored history.
      const history = state.terminalHistoryMap.get(channel_id);
      if (history?.length) history[history.length - 1].output += text;
    }

    if (done) {
      clearTerminalTimers();
      document.getElementById('terminal-kill-btn')?.classList.add('hidden');
      // Update cwd.
      if (cwd) {
        state.terminalCwdMap.set(channel_id, cwd);
        if (state.chatCurrentChannel === channel_id) renderTerminalCwd();
      }
      // Update stored exit code.
      const history = state.terminalHistoryMap.get(channel_id);
      if (history?.length) history[history.length - 1].exitCode = exit_code;
      // Show exit code if non-zero.
      if (state.terminalCurrentBlock && state.terminalCurrentBlock.channelId === channel_id && exit_code !== 0) {
        const exitDiv = document.createElement('div');
        exitDiv.className = 'terminal-cmd-exit error';
        exitDiv.textContent = `exit ${exit_code}`;
        state.terminalCurrentBlock.block.appendChild(exitDiv);
      }
      if (state.terminalCurrentBlock?.channelId === channel_id) {
        state.terminalCurrentBlock = null;
      }
      state.terminalRunning = false;
      // Show prompt row again with updated cwd.
      if (state.chatCurrentChannel === channel_id) {
        const promptRow = document.getElementById('terminal-prompt-row');
        promptRow.classList.remove('hidden');
        const output = document.getElementById('terminal-output');
        output.scrollTop = output.scrollHeight;
        document.getElementById('terminal-input')?.focus();
      }
    }
  });

  instance.addEventListener('terminal_completions', (evt) => {
    state.terminalCompletionPending = false;
    const { completions } = evt.detail;
    if (!completions || !completions.length) return;
    const input = document.getElementById('terminal-input');
    if (!input) return;
    // Use the context saved at request time (not the echoed partial)
    const beforePartial = state.terminalCompletionBase;
    const partial = state.terminalCompletionPartial;

    // Remove any previous completion display
    document.querySelectorAll('.terminal-completions').forEach(el => el.remove());

    if (completions.length === 1) {
      // Single match — substitute it in
      const match = completions[0];
      const suffix = match.endsWith('/') ? '' : ' ';
      input.value = beforePartial + match + suffix;
      clearTerminalCompletions();
    } else {
      // Multiple matches — find common prefix and complete that
      let common = completions[0];
      for (let i = 1; i < completions.length; i++) {
        while (common && !completions[i].startsWith(common)) {
          common = common.slice(0, -1);
        }
      }
      if (common.length > partial.length) {
        input.value = beforePartial + common;
      }
      // Store for Tab cycling
      state.terminalCompletions = completions;
      state.terminalCompletionIndex = -1;
      // Show candidates below the prompt
      const output = document.getElementById('terminal-output');
      if (output) {
        const compDiv = document.createElement('div');
        compDiv.className = 'terminal-completions';
        compDiv.textContent = completions.map(c => c.split('/').filter(Boolean).pop() + (c.endsWith('/') ? '/' : '')).join('  ');
        output.appendChild(compDiv);
        output.scrollTop = output.scrollHeight;
      }
    }
  });

  // ----- Files view events -----
}
