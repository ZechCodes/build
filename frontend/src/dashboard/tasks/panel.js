import { state } from '../state.js';

export function renderTasksPanel(channelId) {
  const list = document.getElementById('tasks-list');
  const empty = document.getElementById('tasks-empty');
  if (!list) return;
  const todos = state.channelTodos.get(channelId) || [];
  list.innerHTML = '';
  if (todos.length === 0) {
    if (empty) empty.classList.remove('hidden');
    return;
  }
  if (empty) empty.classList.add('hidden');
  const completed = todos.filter(t => t.status === 'completed').length;
  const summary = document.createElement('div');
  summary.className = 'tasks-summary';
  const pct = todos.length ? Math.round(completed / todos.length * 100) : 0;
  summary.innerHTML = `<span>${completed}/${todos.length} completed</span><div class="tasks-progress-bar"><div class="tasks-progress-fill" style="width:${pct}%"></div></div>`;
  list.appendChild(summary);
  const order = { in_progress: 0, pending: 1, completed: 2 };
  const sorted = [...todos].sort((a, b) => (order[a.status] ?? 1) - (order[b.status] ?? 1));
  for (const todo of sorted) {
    const row = document.createElement('div');
    row.className = `task-item ${todo.status}`;
    const icon = document.createElement('span');
    icon.className = 'task-status-icon';
    if (todo.status === 'in_progress') {
      icon.innerHTML = '<span class="task-pulse-dot"></span>';
    } else {
      icon.textContent = todo.status === 'completed' ? '✓' : '○';
    }
    const content = document.createElement('span');
    content.className = 'task-content';
    content.textContent = todo.content;
    row.appendChild(icon);
    row.appendChild(content);
    list.appendChild(row);
  }
}

export function updateTasksBadge(channelId) {
  if (state.chatCurrentChannel !== channelId) return;
  const badge = document.getElementById('sidebar-tasks-badge');
  if (!badge) return;
  const todos = state.channelTodos.get(channelId) || [];
  const incomplete = todos.filter(t => t.status !== 'completed').length;
  if (incomplete > 0) {
    badge.textContent = String(incomplete);
    badge.classList.remove('hidden');
    badge.classList.add('accent');
  } else {
    badge.classList.add('hidden');
    badge.classList.remove('accent');
  }
}
