import { state } from '../state.js';
import { escapeHtml } from '../util/html.js';
import { getTerminalCwd } from '../terminal/terminal.js';
import { getActiveE2EE } from '../e2ee/bridge.js';

export function renderComplications() {
  const scroll = document.getElementById('comp-scroll');
  const bar = document.getElementById('complications');
  if (!scroll || !bar) return;
  const channelComps = state.complicationState.get(state.chatCurrentChannel);
  if (!channelComps || channelComps.size === 0) {
    scroll.innerHTML = '';
    bar.classList.add('hidden');
    closeCompPopover();
    return;
  }
  bar.classList.remove('hidden');
  // Filter git complications to only those whose repo is a parent of the channel's working directory.
  const channelWd = getTerminalCwd(state.chatCurrentChannel);
  // Sort by timestamp descending (most recent first).
  const sorted = [...channelComps.values()]
    .filter(comp => {
      if (comp.kind === 'git-status' && channelWd && comp.data?.repo) {
        const repo = comp.data.repo;
        let wd = channelWd;
        // Expand ~ using home dir inferred from the absolute repo path.
        if (wd.startsWith('~/') && repo.startsWith('/')) {
          const homeMatch = repo.match(/^(\/(?:Users|home)\/[^/]+)/);
          if (homeMatch) wd = homeMatch[1] + wd.slice(1);
        }
        const r = repo.endsWith('/') ? repo : repo + '/';
        const w = wd.endsWith('/') ? wd : wd + '/';
        return w.startsWith(r) || r.startsWith(w);
      }
      return true;
    })
    .sort((a, b) => (b.timestamp || 0) - (a.timestamp || 0));
  if (sorted.length === 0) {
    scroll.innerHTML = '';
    bar.classList.add('hidden');
    closeCompPopover();
    return;
  }
  scroll.innerHTML = sorted.map(comp => {
    if (comp.kind === 'git-status') return renderGitCompChip(comp);
    return '';
  }).join('');
  // Attach click handlers.
  scroll.querySelectorAll('[data-comp-id]').forEach(el => {
    el.addEventListener('click', () => toggleCompPopover(el.dataset.compId));
  });
}

export function renderGitCompChip(comp) {
  const d = comp.data || {};
  const branch = escapeHtml(d.branch || '?');
  const remote = d.remote_name ? escapeHtml(d.remote_name) : null;
  const ins = d.insertions || 0;
  const del = d.deletions || 0;
  const gitIcon = `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M6 3v10M10 3v4"/><circle cx="6" cy="14" r="1.5"/><circle cx="10" cy="8" r="1.5"/></svg>`;
  // Line 1: remote/branch
  let line1 = `<span class="comp-line">${gitIcon}`;
  if (remote) line1 += `<span class="val">${remote}</span><span class="v">/</span>`;
  line1 += `<span class="val">${branch}</span></span>`;
  // Line 2: stats (always shown)
  let stats = [];
  stats.push(`<span class="v green">+${ins}</span>`);
  stats.push(`<span class="v red">&minus;${del}</span>`);
  if (d.ahead > 0) stats.push(`<span class="v">&uarr;${d.ahead}</span>`);
  if (d.behind > 0) stats.push(`<span class="v">&darr;${d.behind}</span>`);
  if (d.conflicts > 0) stats.push(`<span class="v red">⚠${d.conflicts}</span>`);
  const line2 = `<span class="comp-line comp-stats">${stats.join('')}</span>`;
  const active = state.compPopoverOpen === comp.id ? ' active' : '';
  return `<div class="comp clickable${active}" data-comp-id="${escapeHtml(comp.id)}">${line1}${line2}</div>`;
}

export function toggleCompPopover(compId) {
  if (state.compPopoverOpen === compId) {
    closeCompPopover();
    return;
  }
  state.compPopoverOpen = compId;
  renderComplications(); // re-render chips to show active state
  const popover = document.getElementById('comp-popover');
  if (!popover) return;
  const channelComps = state.complicationState.get(state.chatCurrentChannel);
  const comp = channelComps?.get(compId);
  if (!comp || comp.kind !== 'git-status') { closeCompPopover(); return; }
  const d = comp.data || {};
  const staged = d.staged || {};
  const unstaged = d.unstaged || {};
  let html = `<div class="cp-section"><div class="cp-label">Branch</div><div class="cp-row"><span class="cp-stat">${escapeHtml(d.branch || '?')}</span>`;
  if (d.upstream) html += ` <span class="text-muted">&rarr; ${escapeHtml(d.upstream)}</span>`;
  html += `</div></div>`;
  html += `<div class="cp-section"><div class="cp-label">Staged</div><div class="cp-row"><span class="cp-stat green">+${staged.added||0}</span> <span class="cp-stat amber">~${staged.modified||0}</span> <span class="cp-stat red">&minus;${staged.deleted||0}</span></div></div>`;
  html += `<div class="cp-section"><div class="cp-label">Unstaged</div><div class="cp-row"><span class="cp-stat green">+${unstaged.added||0}</span> <span class="cp-stat amber">~${unstaged.modified||0}</span> <span class="cp-stat red">&minus;${unstaged.deleted||0}</span></div></div>`;
  html += `<div class="cp-section"><div class="cp-label">Untracked</div><div class="cp-row"><span class="cp-stat">${d.untracked||0}</span></div></div>`;
  html += `<div class="cp-section"><div class="cp-label">Remote</div><div class="cp-row">`;
  html += `<span class="cp-stat">&uarr;${d.ahead||0} ahead</span>`;
  html += `<span class="cp-stat">&darr;${d.behind||0} behind</span>`;
  html += `</div></div>`;
  if (d.last_fetch) {
    const ago = Math.round((Date.now() - d.last_fetch) / 1000);
    const agoStr = ago < 60 ? `${ago}s ago` : ago < 3600 ? `${Math.round(ago/60)}m ago` : `${Math.round(ago/3600)}h ago`;
    html += `<div class="cp-section"><div class="cp-label">Last fetch</div><div class="cp-row">${agoStr}</div></div>`;
  }
  // Action buttons.
  const options = comp.options || [];
  if (options.length) {
    html += `<div class="cp-actions">`;
    for (const opt of options) {
      html += `<button class="cp-btn" data-action="${escapeHtml(opt.id)}" ${opt.enabled ? '' : 'disabled'}>${escapeHtml(opt.label)}</button>`;
    }
    html += `</div>`;
  }
  popover.innerHTML = html;
  popover.classList.remove('hidden');
  // Attach action handlers.
  popover.querySelectorAll('.cp-btn[data-action]').forEach(btn => {
    btn.addEventListener('click', () => {
      if (btn.disabled) return;
      const actionId = btn.dataset.action;
      btn.disabled = true;
      btn.textContent += '…';
      sendComplicationAction(compId, actionId);
    });
  });
}

export function closeCompPopover() {
  state.compPopoverOpen = null;
  const popover = document.getElementById('comp-popover');
  if (popover) { popover.classList.add('hidden'); popover.innerHTML = ''; }
}

export function sendComplicationAction(compId, optionId) {
  const _conn = getActiveE2EE();
  if (!_conn || !_conn.connected || !state.chatCurrentChannel) return;
  _conn.send({
    action: 'complication:action',
    channel_id: state.chatCurrentChannel,
    complication_id: compId,
    option_id: optionId,
  });
}

// Close popover when clicking outside.
document.addEventListener('click', (e) => {
  if (state.compPopoverOpen && !e.target.closest('.comp-popover') && !e.target.closest('.comp[data-comp-id]')) {
    closeCompPopover();
    renderComplications();
  }
});
