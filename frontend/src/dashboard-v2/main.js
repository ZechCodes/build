import {
  AGENTS,
  FILE_TREE,
  INBOX,
  PLAN_DOC,
  PROJECTS,
  SAMPLE_HUNKS,
  WORKTREE,
  allPlans,
  allWorktrees,
  findPlan,
  findProject,
  findWorktree,
} from './data/mock.js';
import { BuildE2EEV1 } from './protocol/v1-client.js';

const state = {
  route: parseRoute(),
  inboxFilter: 'all',
  projectFilter: 'all',
  search: '',
  worktreeTab: 'diff',
  dismissedInbox: new Set(),
  localChat: [],
  terminalLines: [],
  terminalOpen: true,
  chatOpen: true,
  transport: {
    phase: 'checking',
    label: 'Checking devices',
    devices: [],
    readyDevice: null,
    channels: [],
    version: null,
    client: null,
    error: null,
  },
};

let root;

function boot() {
  root = document.getElementById('app');
  if (!root) return;
  root.addEventListener('click', handleClick);
  root.addEventListener('input', handleInput);
  root.addEventListener('submit', handleSubmit);
  window.addEventListener('hashchange', () => {
    state.route = parseRoute();
    render();
  });
  render();
  initTransport();
}

async function initTransport() {
  try {
    setTransport({ phase: 'checking', label: 'Checking devices', error: null });
    const devices = await fetchDevices();
    const readyDevice = devices.find(device => device.status === 'online' && device.has_transport_key);
    if (!readyDevice) {
      setTransport({
        phase: 'mock',
        label: devices.length ? 'No online encrypted device' : 'Mock data',
        devices,
        readyDevice: null,
      });
      return;
    }

    const client = new BuildE2EEV1();
    bindClientEvents(client);
    setTransport({
      phase: 'connecting',
      label: `Connecting to ${readyDevice.name}`,
      devices,
      readyDevice,
      client,
    });

    await client.ready();
    await client.connect(readyDevice.id);
    const hello = await client.hello();
    const channelList = await client.listChannels();
    setTransport({
      phase: 'connected',
      label: `${readyDevice.name} - v${hello.payload?.version || 1}`,
      version: hello.payload?.version || 1,
      channels: channelList.payload?.channels || [],
      error: null,
    });
  } catch (err) {
    setTransport({
      phase: 'mock',
      label: 'Mock data',
      error: String(err?.message || err),
    });
  }
}

async function fetchDevices() {
  const response = await fetch('/api/devices/', { headers: { Accept: 'application/json' } });
  if (!response.ok) throw new Error(`devices fetch failed: ${response.status}`);
  const devices = await response.json();
  if (!Array.isArray(devices)) throw new Error('devices response was not a list');
  return devices;
}

function bindClientEvents(client) {
  client.addEventListener('disconnected', () => {
    setTransport({ phase: 'mock', label: 'Disconnected' });
  });
  client.addEventListener('v1:message.created', event => {
    const message = event.detail?.payload?.message;
    if (!message?.content) return;
    state.localChat.push({
      author: message.sender || 'agent',
      text: message.content,
      time: 'now',
    });
    render();
  });
  client.addEventListener('v1:terminal.exec.output', event => {
    const data = event.detail?.payload?.stdout;
    if (!data) return;
    state.terminalLines.push(data.trimEnd());
    render();
  });
}

function setTransport(patch) {
  state.transport = { ...state.transport, ...patch };
  render();
}

function parseRoute() {
  const raw = window.location.hash.replace(/^#\/?/, '');
  if (!raw) return { screen: 'inbox' };
  const [screen, id] = raw.split('/');
  if (screen === 'project') return { screen: 'project', id };
  if (screen === 'plan') return { screen: 'plan', id };
  if (screen === 'worktree') return { screen: 'worktree', id };
  if (screen === 'projects') return { screen: 'projects' };
  return { screen: 'inbox' };
}

function navigate(path) {
  window.location.hash = path;
}

function handleClick(event) {
  const routeEl = event.target.closest('[data-route]');
  if (routeEl) {
    event.preventDefault();
    navigate(routeEl.dataset.route);
    return;
  }

  const actionEl = event.target.closest('[data-action]');
  if (!actionEl) return;
  event.preventDefault();

  const action = actionEl.dataset.action;
  if (action === 'inbox-filter') {
    state.inboxFilter = actionEl.dataset.value || 'all';
    render();
  } else if (action === 'project-filter') {
    state.projectFilter = actionEl.dataset.value || 'all';
    render();
  } else if (action === 'dismiss-inbox') {
    state.dismissedInbox.add(actionEl.dataset.id);
    render();
  } else if (action === 'worktree-tab') {
    state.worktreeTab = actionEl.dataset.value || 'diff';
    render();
  } else if (action === 'toggle-terminal') {
    state.terminalOpen = !state.terminalOpen;
    render();
  } else if (action === 'toggle-chat') {
    state.chatOpen = !state.chatOpen;
    render();
  } else if (action === 'sync-v1') {
    initTransport();
  }
}

function handleInput(event) {
  const target = event.target;
  if (target.matches('[data-search]')) {
    const cursor = target.selectionStart;
    state.search = target.value;
    render();
    const next = root.querySelector('[data-search]');
    if (next) {
      next.focus();
      next.setSelectionRange(cursor, cursor);
    }
  }
}

async function handleSubmit(event) {
  const form = event.target.closest('form[data-form]');
  if (!form) return;
  event.preventDefault();

  const input = form.querySelector('input[name="message"], input[name="command"]');
  const value = input?.value?.trim();
  if (!value) return;
  input.value = '';

  if (form.dataset.form === 'chat') {
    state.localChat.push({ author: 'You', text: value, time: 'now' });
    const liveChannel = firstLiveChannel();
    if (state.transport.client?.connected && liveChannel) {
      try {
        await state.transport.client.sendMessage(liveChannel.id, value);
      } catch (err) {
        state.localChat.push({ author: 'Build', text: String(err?.message || err), time: 'now' });
      }
    }
    render();
  }

  if (form.dataset.form === 'terminal') {
    state.terminalLines.push(`$ ${value}`);
    const liveChannel = firstLiveChannel();
    if (state.transport.client?.connected && liveChannel) {
      try {
        await state.transport.client.terminalExec(liveChannel.id, value, {
          onStream(frame) {
            const stdout = frame.payload?.stdout;
            if (stdout) state.terminalLines.push(stdout.trimEnd());
            render();
          },
        });
      } catch (err) {
        state.terminalLines.push(String(err?.message || err));
      }
    } else {
      state.terminalLines.push('mock transport: command recorded locally');
    }
    render();
  }
}

function firstLiveChannel() {
  return state.transport.channels[0] || null;
}

function render() {
  if (!root) return;
  root.innerHTML = `
    <div class="dash-shell">
      ${renderSidebar()}
      <main class="dash-main">
        ${renderTopbar()}
        ${renderContent()}
      </main>
    </div>
  `;
}

function renderSidebar() {
  const plansNeedingYou = INBOX.filter(item => !state.dismissedInbox.has(item.id)).length;
  return `
    <aside class="side-nav">
      <div class="brand-block">
        <div class="brand-mark">B</div>
        <div>
          <div class="brand-name">Build</div>
          <div class="brand-meta">${escapeHtml(userName())}</div>
        </div>
      </div>
      <nav class="nav-stack" aria-label="Dashboard">
        ${navButton('Inbox', '#/inbox', state.route.screen === 'inbox', plansNeedingYou)}
        ${navButton('Projects', '#/projects', ['projects', 'project', 'plan', 'worktree'].includes(state.route.screen), PROJECTS.length)}
      </nav>
      <div class="side-section">
        <div class="side-label">Active Work</div>
        ${allWorktrees().slice(0, 4).map(item => `
          <a class="side-worktree" href="#/worktree/${escapeAttr(item.id)}" data-route="#/worktree/${escapeAttr(item.id)}">
            <span class="dot" style="--dot:${escapeAttr(item.project.color)}"></span>
            <span class="truncate">${escapeHtml(item.summary)}</span>
          </a>
        `).join('')}
      </div>
      <div class="transport-card ${state.transport.phase}">
        <div class="transport-row">
          <span class="pulse"></span>
          <span>${escapeHtml(state.transport.label)}</span>
        </div>
        <button class="icon-button" type="button" data-action="sync-v1" aria-label="Refresh v1 transport">R</button>
      </div>
    </aside>
  `;
}

function navButton(label, route, active, count) {
  return `
    <a class="nav-button ${active ? 'active' : ''}" href="${escapeAttr(route)}" data-route="${escapeAttr(route)}">
      <span>${escapeHtml(label)}</span>
      <span class="nav-count">${count}</span>
    </a>
  `;
}

function renderTopbar() {
  const crumbs = breadcrumb();
  return `
    <header class="topbar">
      <div class="crumbs">${crumbs.map(crumb => {
        if (crumb.route) {
          return `<a href="${escapeAttr(crumb.route)}" data-route="${escapeAttr(crumb.route)}">${escapeHtml(crumb.label)}</a>`;
        }
        return `<span>${escapeHtml(crumb.label)}</span>`;
      }).join('<span class="chev">/</span>')}</div>
      <label class="search-box">
        <span>Search</span>
        <input data-search type="search" value="${escapeAttr(state.search)}" placeholder="Projects, plans, worktrees">
      </label>
      <div class="top-actions">
        <span class="status-pill ${state.transport.phase}">${escapeHtml(statusText())}</span>
        <button class="primary small" type="button" data-action="sync-v1">Sync</button>
      </div>
    </header>
  `;
}

function breadcrumb() {
  if (state.route.screen === 'project') {
    const project = findProject(state.route.id);
    return [{ label: 'Projects', route: '#/projects' }, { label: project.name }];
  }
  if (state.route.screen === 'plan') {
    const plan = findPlan(state.route.id);
    return [
      { label: 'Projects', route: '#/projects' },
      { label: plan.project.name, route: `#/project/${plan.project.id}` },
      { label: plan.title },
    ];
  }
  if (state.route.screen === 'worktree') {
    const worktree = findWorktree(state.route.id);
    return [
      { label: 'Projects', route: '#/projects' },
      { label: worktree.project.name, route: `#/project/${worktree.project.id}` },
      { label: worktree.summary },
    ];
  }
  if (state.route.screen === 'projects') return [{ label: 'Projects' }];
  return [{ label: 'Inbox' }];
}

function statusText() {
  if (state.transport.phase === 'connected') {
    return `E2EE v${state.transport.version || 1}`;
  }
  if (state.transport.phase === 'connecting') return 'Connecting';
  if (state.transport.phase === 'checking') return 'Checking';
  return 'Mock data';
}

function renderContent() {
  if (state.route.screen === 'projects') return renderProjects();
  if (state.route.screen === 'project') return renderProject(findProject(state.route.id));
  if (state.route.screen === 'plan') return renderPlan(findPlan(state.route.id));
  if (state.route.screen === 'worktree') return renderWorktree(findWorktree(state.route.id));
  return renderInbox();
}

function renderInbox() {
  const filters = ['all', 'plan-approval', 'permission', 'review', 'question'];
  const visible = INBOX
    .filter(item => !state.dismissedInbox.has(item.id))
    .filter(item => state.inboxFilter === 'all' || item.kind === state.inboxFilter)
    .filter(matchesSearch);

  return `
    <section class="screen inbox-screen">
      <div class="screen-head">
        <div>
          <h1>Inbox</h1>
          <p>${visible.length} items need a decision</p>
        </div>
        <div class="metric-row">
          ${metric('Needs you', INBOX.length - state.dismissedInbox.size)}
          ${metric('Running', sum(PROJECTS, 'runningAgents'))}
          ${metric('Queued', sum(PROJECTS, 'queued'))}
        </div>
      </div>
      <div class="segmented">
        ${filters.map(filter => `
          <button type="button" class="${state.inboxFilter === filter ? 'active' : ''}" data-action="inbox-filter" data-value="${escapeAttr(filter)}">
            ${escapeHtml(labelFor(filter))}
          </button>
        `).join('')}
      </div>
      <div class="inbox-list">
        ${visible.map(renderInboxItem).join('') || renderEmpty('Inbox clear')}
      </div>
    </section>
  `;
}

function renderInboxItem(item) {
  const project = findProject(item.projectId);
  const openRoute = item.worktreeId ? `#/worktree/${item.worktreeId}` : `#/plan/${item.planId}`;
  return `
    <article class="inbox-item priority-${escapeAttr(item.priority)}">
      <div class="inbox-kind">
        <span class="dot" style="--dot:${escapeAttr(project.color)}"></span>
        <span>${escapeHtml(labelFor(item.kind))}</span>
      </div>
      <div class="inbox-body">
        <a class="item-title" href="${escapeAttr(openRoute)}" data-route="${escapeAttr(openRoute)}">${escapeHtml(item.title)}</a>
        <p>${escapeHtml(item.detail)}</p>
        <div class="item-meta">
          <span>${escapeHtml(project.name)}</span>
          <span>${escapeHtml(item.actor)}</span>
          <span>${escapeHtml(item.time)}</span>
        </div>
      </div>
      <div class="item-actions">
        ${item.actions.slice(0, 2).map(action => `
          <button class="${action === 'Approve' || action === 'Allow' ? 'primary' : 'secondary'} small" type="button" data-action="dismiss-inbox" data-id="${escapeAttr(item.id)}">${escapeHtml(action)}</button>
        `).join('')}
      </div>
    </article>
  `;
}

function renderProjects() {
  const filters = ['all', 'needs-you', 'running', 'queued'];
  const projects = PROJECTS
    .filter(project => {
      if (state.projectFilter === 'needs-you') return project.needsYou > 0;
      if (state.projectFilter === 'running') return project.runningAgents > 0;
      if (state.projectFilter === 'queued') return project.queued > 0;
      return true;
    })
    .filter(matchesSearch);

  return `
    <section class="screen projects-screen">
      <div class="screen-head">
        <div>
          <h1>Projects</h1>
          <p>${projects.length} active repositories</p>
        </div>
        <div class="segmented inline">
          ${filters.map(filter => `
            <button type="button" class="${state.projectFilter === filter ? 'active' : ''}" data-action="project-filter" data-value="${escapeAttr(filter)}">
              ${escapeHtml(labelFor(filter))}
            </button>
          `).join('')}
        </div>
      </div>
      <div class="project-grid">
        ${projects.map(project => `
          <article class="project-card" data-route="#/project/${escapeAttr(project.id)}">
            <a class="card-hit" href="#/project/${escapeAttr(project.id)}" data-route="#/project/${escapeAttr(project.id)}" aria-label="${escapeAttr(project.name)}"></a>
            <div class="project-top">
              <span class="project-color" style="--project:${escapeAttr(project.color)}"></span>
              <div>
                <h2>${escapeHtml(project.name)}</h2>
                <p>${escapeHtml(project.description)}</p>
              </div>
            </div>
            <div class="project-meta">
              <span>${escapeHtml(project.repo)}</span>
              <span>${escapeHtml(project.branch)}</span>
              <span>${escapeHtml(project.lastActive)}</span>
            </div>
            <div class="project-stats">
              ${metric('Needs you', project.needsYou)}
              ${metric('Running', project.runningAgents)}
              ${metric('Queued', project.queued)}
            </div>
            <div class="mini-list">
              ${project.worktrees.slice(0, 2).map(wt => `
                <div>
                  ${agentBadge(wt.model)}
                  <span class="truncate">${escapeHtml(wt.summary)}</span>
                  <span class="mini-status ${escapeAttr(wt.status)}">${escapeHtml(wt.status)}</span>
                </div>
              `).join('')}
            </div>
          </article>
        `).join('') || renderEmpty('No projects')}
      </div>
    </section>
  `;
}

function renderProject(project) {
  const lanes = [
    ['in-progress', 'In Progress'],
    ['queued', 'Queued'],
    ['draft', 'Drafts'],
  ];
  return `
    <section class="screen project-screen">
      <div class="project-hero" style="--project:${escapeAttr(project.color)}">
        <div>
          <div class="eyebrow">${escapeHtml(project.repo)} / ${escapeHtml(project.branch)}</div>
          <h1>${escapeHtml(project.name)}</h1>
          <p>${escapeHtml(project.description)}</p>
        </div>
        <div class="metric-row">
          ${metric('Needs you', project.needsYou)}
          ${metric('Running', project.runningAgents)}
          ${metric('Queued', project.queued)}
        </div>
      </div>
      <div class="project-layout">
        <section class="lane-board">
          ${lanes.map(([status, label]) => `
            <div class="lane">
              <div class="lane-title">${escapeHtml(label)} <span>${project.plans.filter(plan => plan.status === status).length}</span></div>
              ${project.plans.filter(plan => plan.status === status).map(plan => renderPlanCard(plan, project)).join('') || renderEmpty('None')}
            </div>
          `).join('')}
        </section>
        <aside class="project-aside">
          <div class="panel">
            <div class="panel-title">Worktrees</div>
            ${project.worktrees.map(wt => `
              <a class="worktree-row" href="#/worktree/${escapeAttr(wt.id)}" data-route="#/worktree/${escapeAttr(wt.id)}">
                ${agentBadge(wt.model)}
                <span class="worktree-main">
                  <span>${escapeHtml(wt.summary)}</span>
                  <span>${escapeHtml(wt.branch)}</span>
                </span>
                <span class="mini-status ${escapeAttr(wt.status)}">${escapeHtml(wt.status)}</span>
              </a>
            `).join('')}
          </div>
          <div class="panel">
            <div class="panel-title">Recent</div>
            <div class="activity-list">
              ${project.activity.map(item => `<div>${escapeHtml(item)}</div>`).join('')}
            </div>
          </div>
        </aside>
      </div>
    </section>
  `;
}

function renderPlanCard(plan, project) {
  const pct = Math.round((plan.doneSteps / Math.max(plan.steps, 1)) * 100);
  return `
    <article class="plan-card" data-route="#/plan/${escapeAttr(plan.id)}">
      <a class="card-hit" href="#/plan/${escapeAttr(plan.id)}" data-route="#/plan/${escapeAttr(plan.id)}" aria-label="${escapeAttr(plan.title)}"></a>
      <div class="plan-card-top">
        ${agentBadge(plan.model)}
        <span>${escapeHtml(plan.updated)}</span>
      </div>
      <h3>${escapeHtml(plan.title)}</h3>
      <div class="progress">
        <span style="width:${pct}%"></span>
      </div>
      <div class="plan-meta">
        <span>${plan.doneSteps}/${plan.steps} steps</span>
        <span>${escapeHtml(project.name)}</span>
      </div>
    </article>
  `;
}

function renderPlan(planItem) {
  const project = planItem.project;
  const doc = planItem.id === PLAN_DOC.id ? PLAN_DOC : { ...PLAN_DOC, id: planItem.id, title: planItem.title };
  const messages = [...doc.chat, ...state.localChat];
  return `
    <section class="screen plan-screen">
      <div class="workspace-head">
        <div>
          <div class="eyebrow">${escapeHtml(project.repo)} / ${escapeHtml(project.branch)}</div>
          <h1>${escapeHtml(doc.title)}</h1>
          <p>${escapeHtml(doc.status)} - ${escapeHtml(doc.updated)}</p>
        </div>
        <div class="workspace-actions">
          <button class="secondary" type="button" data-route="#/project/${escapeAttr(project.id)}">Back</button>
          <button class="primary" type="button">Approve</button>
        </div>
      </div>
      <div class="plan-layout">
        <article class="document-panel">
          ${doc.phases.map((phase, index) => `
            <section class="doc-section">
              <div class="section-number">${index + 1}</div>
              <div>
                <h2>${escapeHtml(phase.title)}</h2>
                <p>${escapeHtml(phase.body)}</p>
                <ol>
                  ${phase.steps.map(step => `<li>${escapeHtml(step)}</li>`).join('')}
                </ol>
              </div>
            </section>
          `).join('')}
        </article>
        <aside class="chat-panel ${state.chatOpen ? '' : 'collapsed'}">
          <div class="panel-title-row">
            <div class="panel-title">Plan Chat</div>
            <button class="icon-button" type="button" data-action="toggle-chat" aria-label="Toggle chat">C</button>
          </div>
          ${state.chatOpen ? renderChat(messages, 'plan') : ''}
        </aside>
      </div>
    </section>
  `;
}

function renderWorktree(worktreeItem) {
  const project = worktreeItem.project;
  const wt = worktreeItem.id === WORKTREE.id ? WORKTREE : { ...WORKTREE, ...worktreeItem };
  const messages = [...WORKTREE.chat, ...state.localChat];
  const tabs = ['diff', 'files', 'git', 'tests'];
  return `
    <section class="screen worktree-screen">
      <div class="workspace-head compact">
        <div>
          <div class="eyebrow">${escapeHtml(project.name)} / ${escapeHtml(wt.branch)}</div>
          <h1>${escapeHtml(wt.title || wt.summary)}</h1>
          <p>${escapeHtml(wt.status)} - ${escapeHtml(wt.device)}</p>
        </div>
        <div class="workspace-actions">
          <button class="secondary" type="button" data-route="#/project/${escapeAttr(project.id)}">Project</button>
          <button class="primary" type="button">Review</button>
        </div>
      </div>
      <div class="worktree-grid">
        <section class="workbench">
          <div class="worktree-summary">
            ${metric('Progress', `${wt.progress || worktreeItem.pct}%`)}
            ${metric('Files', wt.files.length)}
            ${metric('Add', `+${sum(wt.files, 'add')}`)}
            ${metric('Del', `-${sum(wt.files, 'del')}`)}
          </div>
          <div class="tabs">
            ${tabs.map(tab => `
              <button type="button" class="${state.worktreeTab === tab ? 'active' : ''}" data-action="worktree-tab" data-value="${escapeAttr(tab)}">
                ${escapeHtml(labelFor(tab))}
              </button>
            `).join('')}
          </div>
          <div class="tab-panel">
            ${renderWorktreeTab(wt)}
          </div>
          <section class="terminal-panel ${state.terminalOpen ? '' : 'collapsed'}">
            <div class="panel-title-row">
              <div class="panel-title">Terminal</div>
              <button class="icon-button" type="button" data-action="toggle-terminal" aria-label="Toggle terminal">T</button>
            </div>
            ${state.terminalOpen ? renderTerminal() : ''}
          </section>
        </section>
        <aside class="chat-panel worktree-chat ${state.chatOpen ? '' : 'collapsed'}">
          <div class="panel-title-row">
            <div class="panel-title">Agent Chat</div>
            <button class="icon-button" type="button" data-action="toggle-chat" aria-label="Toggle chat">C</button>
          </div>
          ${state.chatOpen ? renderChat(messages, 'worktree') : ''}
        </aside>
      </div>
    </section>
  `;
}

function renderWorktreeTab(wt) {
  if (state.worktreeTab === 'files') return renderFiles(wt);
  if (state.worktreeTab === 'git') return renderGit(wt);
  if (state.worktreeTab === 'tests') return renderTests(wt);
  return renderDiff();
}

function renderDiff() {
  return `
    <div class="diff-view">
      ${SAMPLE_HUNKS.map(hunk => `
        <section class="diff-file">
          <div class="diff-file-head">${escapeHtml(hunk.file)}</div>
          <pre>${hunk.lines.map(line => `<span class="${escapeAttr(line.type)}">${escapeHtml(prefixFor(line.type) + line.text)}</span>`).join('')}</pre>
        </section>
      `).join('')}
    </div>
  `;
}

function renderFiles(wt) {
  return `
    <div class="file-layout">
      <div class="tree-panel">
        ${FILE_TREE.map(node => `
          <div class="tree-row ${node.changed ? 'changed' : ''}" style="--depth:${node.depth}">
            <span>${node.type === 'dir' ? 'dir' : 'file'}</span>
            <span>${escapeHtml(node.path.split('/').pop())}</span>
          </div>
        `).join('')}
      </div>
      <div class="file-list-panel">
        ${wt.files.map(file => `
          <div class="file-row">
            <span>${escapeHtml(file.path)}</span>
            <span>${escapeHtml(file.status)}</span>
            <span class="add">+${file.add}</span>
            <span class="del">-${file.del}</span>
          </div>
        `).join('')}
      </div>
    </div>
  `;
}

function renderGit(wt) {
  return `
    <div class="git-panel">
      <div class="git-summary">
        ${metric('Staged', wt.git.staged)}
        ${metric('Unstaged', wt.git.unstaged)}
      </div>
      ${wt.git.commits.map(commit => `
        <div class="commit-row">
          <code>${escapeHtml(commit.sha)}</code>
          <span>${escapeHtml(commit.message)}</span>
          <span>${escapeHtml(commit.time)}</span>
        </div>
      `).join('')}
    </div>
  `;
}

function renderTests(wt) {
  return `
    <div class="tests-panel">
      ${wt.tests.map(test => `
        <div class="test-row ${escapeAttr(test.status)}">
          <span>${escapeHtml(test.name)}</span>
          <span>${escapeHtml(test.status)}</span>
          <span>${escapeHtml(test.duration)}</span>
        </div>
      `).join('')}
    </div>
  `;
}

function renderChat(messages) {
  return `
    <div class="chat-list">
      ${messages.map(msg => `
        <div class="chat-message ${msg.author === 'You' ? 'mine' : ''}">
          <div class="chat-meta">
            <span>${escapeHtml(msg.author)}</span>
            <span>${escapeHtml(msg.time)}</span>
          </div>
          <p>${escapeHtml(msg.text)}</p>
        </div>
      `).join('')}
    </div>
    <form class="composer" data-form="chat">
      <input name="message" autocomplete="off" placeholder="Message agent">
      <button class="primary small" type="submit">Send</button>
    </form>
  `;
}

function renderTerminal() {
  const lines = [...WORKTREE.terminal, ...state.terminalLines];
  return `
    <pre class="terminal-output">${lines.map(line => escapeHtml(line)).join('\n')}</pre>
    <form class="terminal-input" data-form="terminal">
      <input name="command" autocomplete="off" placeholder="Command">
      <button class="primary small" type="submit">Run</button>
    </form>
  `;
}

function metric(label, value) {
  return `
    <div class="metric">
      <span>${escapeHtml(String(value))}</span>
      <span>${escapeHtml(label)}</span>
    </div>
  `;
}

function agentBadge(model) {
  const agent = AGENTS[model] || AGENTS.cs;
  return `
    <span class="agent-badge" style="--agent:${escapeAttr(agent.color)}" title="${escapeAttr(agent.name)}">
      ${escapeHtml(agent.label)}
    </span>
  `;
}

function renderEmpty(label) {
  return `<div class="empty-state">${escapeHtml(label)}</div>`;
}

function matchesSearch(item) {
  const query = state.search.trim().toLowerCase();
  if (!query) return true;
  return JSON.stringify(item).toLowerCase().includes(query);
}

function labelFor(value) {
  return String(value)
    .split('-')
    .map(part => part ? part[0].toUpperCase() + part.slice(1) : part)
    .join(' ');
}

function sum(items, key) {
  return items.reduce((total, item) => total + (Number(item[key]) || 0), 0);
}

function prefixFor(type) {
  if (type === 'add') return '+ ';
  if (type === 'del') return '- ';
  return '  ';
}

function userName() {
  return document.body.dataset.userName || 'Admin';
}

function escapeHtml(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#039;');
}

function escapeAttr(value) {
  return escapeHtml(value);
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', boot);
} else {
  boot();
}
