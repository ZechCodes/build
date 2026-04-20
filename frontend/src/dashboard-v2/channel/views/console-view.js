// ConsoleView — agent activity feed (tool uses + reasoning blocks).
// Mounts into #v2-rail-body. Instance-scoped scroll state.
// See planning/dashboard-v2/05-views.md § ConsoleView.

import { activityStore } from '../../domain/activity-store.js';
import { escapeHtml } from '../../util/html.js';
import { renderMarkdown } from '../../util/markdown.js';
import { describeToolUse, formatToolDetail, formatToolResult, toolTag } from '../../util/tools.js';
import { shortTime } from '../../util/time.js';

export class ConsoleView {
  constructor(channel) {
    this.channel = channel;
    this.root = null;
    this.body = null;
    this.unsubs = [];
    // Scroll + reasoning state is per-instance so it doesn't leak between channels.
    this._lastRenderedCount = 0;
  }

  activate() {
    this.root = document.getElementById('v2-rail-body');
    if (!this.root) return;
    this._buildShell();
    this._render();
    this.unsubs.push(activityStore.subscribe(e => {
      if (e.channelId === this.channel.id) this._render();
    }));
    this.body?.addEventListener('click', this._onClick);
    this.root.querySelector('.v2-console-bubble')?.addEventListener('click', () => this._scrollToBottom(true));
    this.body?.addEventListener('scroll', this._onScroll);
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    this.root = null;
    this.body = null;
    this._lastRenderedCount = 0;
  }

  _buildShell() {
    this.root.innerHTML = `
      <div class="v2-console">
        <div class="v2-console-list" data-slot="list"></div>
        <button class="v2-console-bubble" type="button" hidden>↓ New activity</button>
      </div>
    `;
    this.body = this.root.querySelector('[data-slot="list"]');
  }

  _render() {
    if (!this.body) return;
    const wasNearBottom = this._isNearBottom();
    const entries = activityStore.forChannel(this.channel.id);

    if (entries.length === 0) {
      this.body.innerHTML = '<div class="v2-console-empty">No activity yet.</div>';
      this._lastRenderedCount = 0;
      return;
    }

    this.body.innerHTML = '';
    for (const e of entries) {
      if (e.type === 'tool_use') this.body.appendChild(this._renderToolEntry(e));
      else if (e.type === 'reasoning') this.body.appendChild(this._renderReasoningEntry(e));
    }

    if (this._lastRenderedCount === 0 || wasNearBottom) {
      this._scrollToBottom(false);
    } else {
      this._showBubble(entries.length > this._lastRenderedCount);
    }
    this._lastRenderedCount = entries.length;
  }

  _renderToolEntry(e) {
    const div = document.createElement('div');
    div.className = 'v2-ce';
    if (e.toolUseId) div.dataset.toolUseId = e.toolUseId;
    const tag = toolTag(e.name);
    const desc = describeToolUse(e.name, e.input);
    const time = e.at ? shortTime(e.at) : '';
    const result = e.result
      ? `<div class="v2-ce-result ${e.result.isError ? 'error' : ''}">${formatToolResult(e.name, e.result.content, e.result.isError)}</div>`
      : '';
    div.innerHTML = `
      <div class="v2-ce-row" data-toggle="1">
        <span class="v2-ce-tag ${escapeHtml(tag)}">${escapeHtml(e.name)}</span>
        <span class="v2-ce-desc">${escapeHtml(desc)}</span>
        <span class="v2-ce-time">${escapeHtml(time)}</span>
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
    const time = e.at ? shortTime(e.at) : '';
    div.innerHTML = `
      <div class="v2-ce-row" data-toggle="1">
        <span class="v2-ce-tag reasoning">Reasoning</span>
        <span class="v2-ce-desc">${escapeHtml(firstLine)}</span>
        <span class="v2-ce-time">${escapeHtml(time)}</span>
      </div>
      <div class="v2-ce-detail v2-ce-reasoning-detail" hidden>${renderMarkdown(e.text || '')}</div>
    `;
    return div;
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

  _onScroll = () => {
    if (!this.body) return;
    if (this._isNearBottom()) {
      const bubble = this.root?.querySelector('.v2-console-bubble');
      if (bubble) bubble.hidden = true;
    }
  };

  _isNearBottom() {
    if (!this.body) return true;
    return this.body.scrollHeight - (this.body.scrollTop + this.body.clientHeight) < 40;
  }

  _showBubble(visible) {
    const bubble = this.root?.querySelector('.v2-console-bubble');
    if (!bubble) return;
    bubble.hidden = !visible;
  }

  _scrollToBottom(_force) {
    if (!this.body) return;
    this.body.scrollTop = this.body.scrollHeight;
    const bubble = this.root?.querySelector('.v2-console-bubble');
    if (bubble) bubble.hidden = true;
  }
}
