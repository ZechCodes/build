// TasksView — sidebar Tasks section. Shell view (single fixed slot),
// reads uiStore.activeChannel + tasksStore. See
// planning/dashboard/05-views.md for the shell-view pattern.

import { tasksStore } from '../domain/tasks-store.js';
import { uiStore } from '../domain/ui-store.js';
import { escapeHtml } from '../util/html.js';

const STATUS_DOTS = {
  pending:     'v2-task-dot pending',
  in_progress: 'v2-task-dot in-progress',
  completed:   'v2-task-dot completed',
};
const STATUS_LABEL = {
  pending: '○', in_progress: '●', completed: '✓',
};

export class TasksView {
  constructor() {
    this.body = null;
    this.badge = null;
    this.unsubs = [];
  }

  activate() {
    this.body = document.getElementById('v2-tasks-body');
    this.badge = document.getElementById('v2-tasks-badge-slot');
    if (!this.body) return;
    this.render();
    this.unsubs.push(uiStore.subscribe(e => { if (e.kind === 'active_channel') this.render(); }));
    this.unsubs.push(tasksStore.subscribe(e => {
      if (e.channelId === uiStore.getActiveChannel()) this.render();
    }));
  }

  deactivate() {
    this.unsubs.forEach(fn => fn());
    this.unsubs = [];
    this.body = null;
    this.badge = null;
  }

  render() {
    if (!this.body) return;
    const channelId = uiStore.getActiveChannel();
    const todos = channelId ? tasksStore.forChannel(channelId) : [];

    if (!todos.length) {
      this.body.innerHTML = '<div class="v2-tasks-empty">No tasks yet.</div>';
    } else {
      this.body.innerHTML = todos.map(t => `
        <div class="v2-task-row" data-status="${escapeHtml(t.status || 'pending')}">
          <span class="${STATUS_DOTS[t.status] || STATUS_DOTS.pending}" aria-hidden="true">${STATUS_LABEL[t.status] || '○'}</span>
          <span class="v2-task-text">${escapeHtml(t.activeForm || t.content || t.text || '')}</span>
        </div>
      `).join('');
    }
    this._renderBadge(todos);
  }

  _renderBadge(todos) {
    if (!this.badge) return;
    if (!todos.length) { this.badge.innerHTML = ''; return; }
    const done = todos.filter(t => t.status === 'completed').length;
    this.badge.innerHTML = `<span class="v2-tasks-badge">${done}/${todos.length}</span>`;
  }
}
