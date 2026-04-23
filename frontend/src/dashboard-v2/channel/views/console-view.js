// ConsoleView — agent activity feed (tool uses + reasoning blocks).
// Mounts into #v2-activity-body (sidebar Activity panel).
// Auto-scrolls to the latest entry on each change. Timestamps tick
// every second (1s / 23m / hh:mm once past an hour).

import { activityStore } from '../../domain/activity-store.js';
import { escapeHtml } from '../../util/html.js';
import { renderMarkdown } from '../../util/markdown.js';
import { describeToolUse, formatToolDetail, formatToolResult, toolTag } from '../../util/tools.js';
import { relativeOrClock } from '../../util/time.js';

export class ConsoleView {
  constructor(channel) {
    this.channel = channel;
    this.root = null;
    this.body = null;
    this.unsubs = [];
    this._timeTicker = null;
  }

  activate() {
    this.root = document.getElementById('v2-activity-body');
    if (!this.root) return;
    this._buildShell();
    this._render();
    this.unsubs.push(activityStore.subscribe(e => {
      if (e.channelId === this.channel.id) this._render();
    }));
    this.body?.addEventListener('click', this._onClick);

    // Tick relative timestamps every second.
    this._timeTicker = setInterval(() => this._tickTimes(), 1000);
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    if (this._timeTicker) { clearInterval(this._timeTicker); this._timeTicker = null; }
    if (this._resizeObs) { this._resizeObs.disconnect(); this._resizeObs = null; }
    this.root = null;
    this.body = null;
  }

  _buildShell() {
    this.root.innerHTML = `
      <div class="v2-console">
        <div class="v2-console-list" data-slot="list"></div>
        <div class="v2-console-skeleton" aria-hidden="true">
          <div class="v2-skeleton v2-skeleton-row"></div>
          <div class="v2-skeleton v2-skeleton-row"></div>
          <div class="v2-skeleton v2-skeleton-row short"></div>
        </div>
      </div>
    `;
    this.body = this.root.querySelector('[data-slot="list"]');
    // Keep pinned to latest whenever the panel changes size (e.g. the
    // sidebar Activity section expands from collapsed).
    if (typeof ResizeObserver !== 'undefined' && this.body) {
      this._resizeObs = new ResizeObserver(() => this._scrollToBottom());
      this._resizeObs.observe(this.body);
    }
  }

  _render() {
    if (!this.body) return;
    const entries = activityStore.forChannel(this.channel.id);

    if (entries.length === 0) {
      this.body.innerHTML = '<div class="v2-console-empty">No activity yet.</div>';
      return;
    }

    this.body.innerHTML = '';
    for (const e of entries) {
      if (e.type === 'tool_use') this.body.appendChild(this._renderToolEntry(e));
      else if (e.type === 'reasoning') this.body.appendChild(this._renderReasoningEntry(e));
    }

    // Auto-scroll latest into view.
    this._scrollToBottom();
  }

  _renderToolEntry(e) {
    const div = document.createElement('div');
    div.className = 'v2-ce';
    if (e.toolUseId) div.dataset.toolUseId = e.toolUseId;
    const tag = toolTag(e.name);
    const desc = describeToolUse(e.name, e.input);
    const result = e.result
      ? `<div class="v2-ce-result ${e.result.isError ? 'error' : ''}">${formatToolResult(e.name, e.result.content, e.result.isError)}</div>`
      : '';
    const atMs = this._toMs(e.at);
    div.innerHTML = `
      <div class="v2-ce-row" data-toggle="1">
        <span class="v2-ce-tag ${escapeHtml(tag)}">${escapeHtml(e.name)}</span>
        <span class="v2-ce-desc">${escapeHtml(desc)}</span>
        <span class="v2-ce-time" data-ts="${atMs || ''}">${escapeHtml(relativeOrClock(e.at))}</span>
      </div>
      <div class="v2-ce-detail" hidden>
        ${formatToolDetail(e.name, e.input)}
        ${result}
      </div>
    `;
    return div;
  }

  _renderReasoningEntry(e) {
    const div = document.createElement('div');
    div.className = 'v2-ce v2-ce-reasoning';
    const firstLine = (e.text || '').split('\n').find(l => l.trim()) || (e.text || '').slice(0, 120);
    const atMs = this._toMs(e.at);
    div.innerHTML = `
      <div class="v2-ce-row" data-toggle="1">
        <span class="v2-ce-tag reasoning">Reasoning</span>
        <span class="v2-ce-desc">${escapeHtml(firstLine)}</span>
        <span class="v2-ce-time" data-ts="${atMs || ''}">${escapeHtml(relativeOrClock(e.at))}</span>
      </div>
      <div class="v2-ce-detail v2-ce-reasoning-detail" hidden>${renderMarkdown(e.text || '')}</div>
    `;
    return div;
  }

  _tickTimes() {
    if (!this.body) return;
    const spans = this.body.querySelectorAll('.v2-ce-time[data-ts]');
    for (const el of spans) {
      const ts = Number(el.getAttribute('data-ts'));
      if (!ts) continue;
      el.textContent = relativeOrClock(ts);
    }
  }

  _toMs(at) {
    if (!at) return 0;
    if (typeof at === 'number') return at < 1e12 ? at * 1000 : at;
    const t = Date.parse(at);
    return Number.isNaN(t) ? 0 : t;
  }

  _onClick = (ev) => {
    const row = ev.target.closest('.v2-ce-row[data-toggle]');
    if (!row) return;
    const entry = row.parentElement;
    const detail = entry.querySelector('.v2-ce-detail');
    if (!detail) return;
    detail.hidden = !detail.hidden;
    entry.classList.toggle('open', !detail.hidden);
  };

  _scrollToBottom() {
    if (!this.body) return;
    // Always pin to bottom after a render — user explicitly wants the
    // latest activity visible.
    this.body.scrollTop = this.body.scrollHeight;
  }
}
