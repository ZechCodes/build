// Tool-use rendering helpers. Ported from v1 console/tools.js.

import { escapeHtml } from './html.js';

export function agentShortName(sender) {
  const map = { 'Claude Code': 'Claude', 'Codex CLI': 'Codex', 'Gemini CLI': 'Gemini' };
  return map[sender] || sender || 'Device';
}

export function describeToolUse(name, input) {
  if (input?.description) return input.description;
  const descs = {
    Read:       () => (input.file_path || '').split('/').pop() || 'file',
    Edit:       () => (input.file_path || '').split('/').pop() || 'file',
    Write:      () => (input.file_path || '').split('/').pop() || 'file',
    Bash:       () => (input.command || '').substring(0, 80),
    Glob:       () => input.pattern || '',
    Grep:       () => `'${input.pattern || ''}'`,
    Agent:      () => input.prompt ? input.prompt.substring(0, 80) : '',
    WebFetch:   () => input.url || 'web page',
    WebSearch:  () => input.query || '',
    ToolSearch: () => input.query || '',
  };
  const fn = descs[name];
  return fn ? fn() : name;
}

export function formatToolDetail(name, input) {
  const parts = [];
  if (name === 'Bash' && input.command) {
    parts.push(`<div class="v2-ce-label">Command</div><pre class="v2-ce-code">${escapeHtml(input.command)}</pre>`);
  } else if (name === 'Edit' && input.file_path) {
    parts.push(`<div class="v2-ce-label">File</div><div class="v2-ce-val">${escapeHtml(input.file_path)}</div>`);
    if (input.old_string != null && input.new_string != null) {
      parts.push(`<div class="v2-ce-label">Diff</div><pre class="v2-ce-diff"><span class="v2-ce-del">${escapeHtml(input.old_string)}</span><span class="v2-ce-add">${escapeHtml(input.new_string)}</span></pre>`);
    }
  } else if ((name === 'Read' || name === 'Write') && input.file_path) {
    parts.push(`<div class="v2-ce-label">File</div><div class="v2-ce-val">${escapeHtml(input.file_path)}</div>`);
  } else if (name === 'Grep') {
    parts.push(`<div class="v2-ce-label">Pattern</div><div class="v2-ce-val">${escapeHtml(input.pattern || '')}</div>`);
    if (input.path) parts.push(`<div class="v2-ce-label">Path</div><div class="v2-ce-val">${escapeHtml(input.path)}</div>`);
  } else if (name === 'Glob') {
    parts.push(`<div class="v2-ce-label">Pattern</div><div class="v2-ce-val">${escapeHtml(input.pattern || '')}</div>`);
  } else if (name === 'Agent' && input.prompt) {
    parts.push(`<div class="v2-ce-label">Prompt</div><pre class="v2-ce-code">${escapeHtml(input.prompt)}</pre>`);
  }
  return parts.join('') || `<pre class="v2-ce-code">${escapeHtml(JSON.stringify(input || {}, null, 2))}</pre>`;
}

export function formatToolResult(name, content, isError) {
  if (!content) return '';
  const text = typeof content === 'string' ? content : JSON.stringify(content, null, 2);
  if (isError) {
    return `<div class="v2-ce-label v2-ce-error-label">Error</div><pre class="v2-ce-code v2-ce-error">${escapeHtml(text)}</pre>`;
  }
  if (name === 'Edit') {
    return `<div class="v2-ce-label">Result</div><div class="v2-ce-val">${escapeHtml(text.substring(0, 200))}</div>`;
  }
  if (name === 'Bash') {
    return `<div class="v2-ce-label">Output</div><pre class="v2-ce-code">${escapeHtml(text)}</pre>`;
  }
  if (name === 'Read') {
    return `<div class="v2-ce-label">Content</div><pre class="v2-ce-code">${escapeHtml(text.length > 2000 ? text.substring(0, 2000) + '\n…truncated' : text)}</pre>`;
  }
  if (text.length > 500) {
    return `<div class="v2-ce-label">Result</div><pre class="v2-ce-code">${escapeHtml(text.substring(0, 500) + '\n…truncated')}</pre>`;
  }
  return `<div class="v2-ce-label">Result</div><pre class="v2-ce-code">${escapeHtml(text)}</pre>`;
}

export function toolTag(name) {
  const map = { Read: 'read', Edit: 'edit', Write: 'write', Bash: 'bash', Grep: 'read', Glob: 'read', Agent: 'bash', ToolSearch: 'read' };
  return map[name] || 'read';
}
