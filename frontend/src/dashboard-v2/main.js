import {
  AGENTS,
  FILE_TREE,
  INBOX,
  PLAN_DOC,
  PROJECTS,
  SAMPLE_HUNKS,
  WORKTREE,
} from './data/mock.js';
import { BuildE2EEV1 } from './protocol/v1-client.js';

const REVIEW_DENIAL_BEGIN = '<build-review-denied>';
const REVIEW_DENIAL_END = '</build-review-denied>';
const REVIEW_DIFF_PREVIEW_LIMIT = 8000;

const state = {
  route: parseRoute(),
  inboxFilter: 'all',
  projectFilter: 'all',
  search: '',
  worktreeTab: 'diff',
  dismissedInbox: new Set(),
  localChatByKey: new Map(),
  terminalLines: [],
  terminalOpen: true,
  chatOpen: true,
  liveSnapshot: null,
  worktreeSnapshots: new Map(),
  worktreeLoads: new Set(),
  selectedFileByWorktree: new Map(),
  fileReads: new Map(),
  fileDiffs: new Map(),
  fileLoads: new Set(),
  fileErrors: new Map(),
  messagesByChannel: new Map(),
  messageLoads: new Set(),
  activityByChannel: new Map(),
  activityLoads: new Set(),
  interactionLoads: new Set(),
  interactionErrors: new Map(),
  reposByProject: new Map(),
  repoLoads: new Set(),
  repoErrors: new Map(),
  spawnLoads: new Set(),
  worktreeActionLoads: new Set(),
  worktreeErrors: new Map(),
  snapshotRefreshTimers: new Map(),
  harnesses: [],
  dialog: null,
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
  root.addEventListener('change', handleChange);
  root.addEventListener('submit', handleSubmit);
  window.addEventListener('hashchange', () => {
    state.route = parseRoute();
    render();
    loadRouteData();
  });
  render();
  initTransport();
}

async function initTransport() {
  try {
    state.worktreeSnapshots.clear();
    state.worktreeLoads.clear();
    state.selectedFileByWorktree.clear();
    state.fileReads.clear();
    state.fileDiffs.clear();
    state.fileLoads.clear();
    state.fileErrors.clear();
    state.messagesByChannel.clear();
    state.messageLoads.clear();
    state.activityByChannel.clear();
    state.activityLoads.clear();
    state.interactionLoads.clear();
    state.interactionErrors.clear();
    state.reposByProject.clear();
    state.repoLoads.clear();
    state.repoErrors.clear();
    setTransport({ phase: 'checking', label: 'Checking devices', error: null });
    const devices = await fetchDevices();
    const readyDevice = devices.find(device => device.status === 'online' && device.has_transport_key);
    if (!readyDevice) {
      state.liveSnapshot = null;
      setTransport({
        phase: 'mock',
        label: devices.length ? 'No online encrypted device' : 'Mock data',
        devices,
        readyDevice: null,
        channels: [],
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
    const [channelList, harnessList] = await Promise.all([
      client.listChannels(),
      client.listHarnesses().catch(() => ({ payload: { harnesses: [] } })),
    ]);
    const snapshot = await loadDashboardSnapshot(client);
    state.liveSnapshot = snapshot;
    state.harnesses = normalizeHarnesses(harnessList.payload?.harnesses);
    setTransport({
      phase: 'connected',
      label: `${readyDevice.name} - v${hello.payload?.version || 1}`,
      version: hello.payload?.version || 1,
      channels: channelList.payload?.channels || [],
      error: null,
    });
    loadRouteData();
  } catch (err) {
    state.liveSnapshot = null;
    state.worktreeSnapshots.clear();
    state.worktreeLoads.clear();
    state.selectedFileByWorktree.clear();
    state.fileReads.clear();
    state.fileDiffs.clear();
    state.fileLoads.clear();
    state.fileErrors.clear();
    state.messagesByChannel.clear();
    state.messageLoads.clear();
    state.activityByChannel.clear();
    state.activityLoads.clear();
    state.interactionLoads.clear();
    state.interactionErrors.clear();
    state.reposByProject.clear();
    state.repoLoads.clear();
    state.repoErrors.clear();
    setTransport({
      phase: 'mock',
      label: 'Mock data',
      error: String(err?.message || err),
    });
  }
}

function loadRouteData() {
  if (state.route.screen === 'project') {
    if (findProjectItem(state.route.id)) loadProjectRepos(state.route.id);
  }
  if (state.route.screen === 'worktree') {
    loadWorktreeSnapshot(state.route.id);
    const worktree = findWorktreeItem(state.route.id);
    if (worktree?.id) {
      const selected = selectedFileFor(worktree);
      if (selected) loadWorktreeFile(worktree.id, selected.path);
    }
  }
  loadRouteMessages();
  loadRouteActivity();
}

async function loadDashboardSnapshot(client) {
  try {
    const [response, inboxResponse] = await Promise.all([
      client.dashboardSnapshot(),
      client.inboxList().catch(err => {
        console.debug('inbox.list unavailable', err);
        return { payload: { items: [] } };
      }),
    ]);
    const projects = response.payload?.projects;
    if (!Array.isArray(projects)) return null;
    const normalizedProjects = projects.map(normalizeLiveProject).filter(Boolean);
    return {
      ...response.payload,
      projects: normalizedProjects,
      inbox: normalizeLiveInbox(inboxResponse.payload?.items, normalizedProjects),
    };
  } catch (err) {
    console.debug('dashboard.snapshot unavailable', err);
    return null;
  }
}

function normalizeLiveProject(project) {
  if (!project?.id || !project?.name) return null;
  const worktrees = Array.isArray(project.worktrees) ? project.worktrees : [];
  const plans = Array.isArray(project.plans) ? project.plans : [];
  const normalizedWorktrees = worktrees.map((worktree, index) => ({
    id: worktree.id || worktree.channel_id || `worktree-${index}`,
    channel_id: worktree.channel_id || worktree.id || '',
    branch: worktree.branch || project.branch || 'workspace',
    plan: worktree.plan || '',
    model: worktree.model || worktree.agent || 'device',
    agent: worktree.agent || worktree.model || 'device',
    status: worktree.status || 'idle',
    harness: worktree.harness || worktree.agent_config?.harness || '',
    effort: worktree.effort || worktree.agent_config?.effort || '',
    auto_approve_tools: !!(worktree.auto_approve_tools ?? worktree.agent_config?.auto_approve_tools),
    agent_running: !!worktree.agent_running,
    agent_status: worktree.agent_status || worktree.agent_config?.status || '',
    agent_config: worktree.agent_config || null,
    summary: worktree.summary || worktree.name || 'Agent workspace',
    device: worktree.device || 'local',
    workspace: worktree.workspace || '',
    pct: Number(worktree.pct) || 0,
    files: Number(worktree.files) || 0,
    add: Number(worktree.add) || 0,
    del: Number(worktree.del) || 0,
    updated: worktree.updated || project.lastActive || 'now',
  }));
  const normalizedPlans = plans.map((plan, index) => ({
    id: plan.id || `plan-${index}`,
    channel_id: plan.channel_id || '',
    title: plan.title || 'Untitled plan',
    status: plan.status || 'draft',
    steps: Number(plan.steps) || Number(plan.step_count) || 1,
    doneSteps: Number(plan.doneSteps) || Number(plan.done_step_count) || 0,
    model: plan.model || 'device',
    updated: plan.updated || project.lastActive || 'now',
  }));
  const runningAgents = normalizedWorktrees.filter(worktree => (
    worktree.agent_running
    || ['active', 'running', 'working', 'in-progress'].includes(String(worktree.agent_status || worktree.status).toLowerCase())
  )).length;
  const queued = normalizedPlans.filter(plan => plan.status === 'queued').length;
  return {
    description: project.root_path || project.repo || 'Project workspace',
    repo: project.name,
    branch: 'main',
    color: colorFor(project.id),
    needsYou: 0,
    lastActive: 'now',
    ...project,
    runningAgents: Number(project.runningAgents ?? project.running_agents ?? runningAgents) || runningAgents,
    queued: Number(project.queued ?? queued) || queued,
    worktrees: normalizedWorktrees,
    plans: normalizedPlans,
    activity: Array.isArray(project.activity) ? project.activity : [],
  };
}

function normalizeLiveInbox(items, projects = []) {
  if (!Array.isArray(items)) return [];
  const projectById = new Map(projects.map(project => [project.id, project]));
  return items.map(item => normalizeLiveInboxItem(item, projectById)).filter(Boolean);
}

function normalizeLiveInboxItem(item, projectById) {
  if (!item?.id || !item?.title) return null;
  const projectId = item.projectId || item.project_id || '';
  const worktreeId = item.worktreeId || item.worktree_id || '';
  const planId = item.planId || item.plan_id || '';
  const channelId = item.channelId || item.channel_id || '';
  const interactionId = item.interactionId || item.interaction_id || '';
  const project = projectById.get(projectId);
  const kind = item.kind || 'review';
  const priority = item.priority || (kind === 'permission' ? 'high' : 'medium');
  return {
    id: item.id,
    kind,
    priority,
    projectId: projectId || project?.id || '',
    projectName: item.projectName || project?.name || 'Project',
    projectColor: item.projectColor || project?.color || colorFor(projectId || item.id),
    worktreeId,
    planId,
    channelId,
    interactionId,
    title: item.title,
    detail: item.detail || '',
    actor: item.actor || 'device',
    time: item.time || 'now',
    actions: Array.isArray(item.actions) && item.actions.length ? item.actions : ['Open'],
    interaction: normalizeInteractionData(item.interaction, interactionId),
  };
}

async function loadProjectRepos(projectId, { force = false } = {}) {
  if (!projectId || !state.transport.client?.connected) return;
  if (!force && (state.reposByProject.has(projectId) || state.repoLoads.has(projectId))) return;
  state.repoLoads.add(projectId);
  state.repoErrors.delete(projectId);
  try {
    const response = await state.transport.client.listProjectRepos(projectId);
    const repos = normalizeProjectRepos(response.payload?.repos);
    state.reposByProject.set(projectId, repos);
    if (state.dialog?.type === 'worktree-create' && state.dialog.projectId === projectId && !state.dialog.values.repo_path) {
      state.dialog.values.repo_path = repos[0]?.path || '';
    }
    render();
  } catch (err) {
    state.repoErrors.set(projectId, String(err?.message || err));
    if (state.dialog?.type === 'worktree-create' && state.dialog.projectId === projectId) {
      state.dialog.error = String(err?.message || err);
    }
    render();
  } finally {
    state.repoLoads.delete(projectId);
    render();
  }
}

function normalizeProjectRepos(repos) {
  if (!Array.isArray(repos)) return [];
  return repos.map(repo => ({
    id: repo.id || repo.path || repo.relative_path || repo.name,
    name: repo.name || repo.remote || repo.relative_path || repo.path || 'repository',
    path: repo.path || '',
    relative_path: repo.relative_path || repo.path || '',
    branch: repo.branch || 'main',
    upstream: repo.upstream || '',
    remote: repo.remote || '',
    is_root: !!repo.is_root,
  })).filter(repo => repo.path);
}

async function loadWorktreeSnapshot(worktreeId, { force = false } = {}) {
  if (!worktreeId || !state.transport.client?.connected) return;
  if (!force && (state.worktreeSnapshots.has(worktreeId) || state.worktreeLoads.has(worktreeId))) return;
  state.worktreeLoads.add(worktreeId);
  try {
    const response = await state.transport.client.worktreeSnapshot(worktreeId);
    const snapshot = normalizeWorktreeSnapshot(response.payload);
    if (snapshot) {
      state.worktreeSnapshots.set(worktreeId, snapshot);
      ensureSelectedFile(worktreeId, snapshot.files);
      const selectedPath = state.selectedFileByWorktree.get(worktreeId);
      if (selectedPath) loadWorktreeFile(worktreeId, selectedPath);
      render();
    }
  } catch (err) {
    console.debug('worktree.snapshot unavailable', err);
  } finally {
    state.worktreeLoads.delete(worktreeId);
  }
}

async function loadRouteMessages() {
  const channelId = currentRouteChannelId();
  if (!channelId || !state.transport.client?.connected) return;
  if (state.messagesByChannel.has(channelId) || state.messageLoads.has(channelId)) return;
  state.messageLoads.add(channelId);
  try {
    const response = await state.transport.client.listMessages(channelId, { limit: 50 });
    const messages = Array.isArray(response.payload?.messages)
      ? response.payload.messages.map(normalizeChatMessage).filter(Boolean)
      : [];
    state.messagesByChannel.set(channelId, messages);
    render();
  } catch (err) {
    console.debug('message.list unavailable', err);
  } finally {
    state.messageLoads.delete(channelId);
  }
}

async function loadRouteActivity() {
  const channelId = currentRouteChannelId();
  if (!channelId || !state.transport.client?.connected) return;
  if (state.activityByChannel.has(channelId) || state.activityLoads.has(channelId)) return;
  state.activityLoads.add(channelId);
  try {
    const response = await state.transport.client.listActivity(channelId);
    const entries = Array.isArray(response.payload?.entries)
      ? response.payload.entries.map(entry => normalizeActivityEntry(entry, channelId)).filter(Boolean)
      : [];
    state.activityByChannel.set(channelId, entries);
    render();
  } catch (err) {
    console.debug('activity.list unavailable', err);
  } finally {
    state.activityLoads.delete(channelId);
  }
}

function normalizeWorktreeSnapshot(payload) {
  if (!payload?.worktree?.id) return null;
  const git = payload.git || {};
  const agent = payload.agent || {};
  return {
    id: payload.worktree.id,
    branch: payload.worktree.branch || git.branch || 'workspace',
    status: payload.worktree.status || 'idle',
    workspace: payload.workspace || payload.worktree.path || '',
    agent_config: agent,
    harness: agent.harness || '',
    model: agent.model || '',
    effort: agent.effort || '',
    auto_approve_tools: !!agent.auto_approve_tools,
    agent_running: !!agent.is_running,
    agent_status: agent.status || '',
    files: Array.isArray(payload.files) ? payload.files.map(file => ({
      path: file.path || '',
      status: file.status || 'M',
      add: Number(file.add) || 0,
      del: Number(file.del) || 0,
    })) : [],
    git: {
      repo_path: git.repo_path || '',
      branch: git.branch || '',
      upstream: git.upstream || '',
      staged: Number(git.staged) || 0,
      unstaged: Number(git.unstaged) || 0,
      error: git.error || null,
      commits: Array.isArray(git.commits) ? git.commits.map(commit => ({
        sha: commit.sha || '',
        message: commit.message || '',
        time: commit.time || '',
      })) : [],
    },
    diffHunks: Array.isArray(payload.diffs) ? payload.diffs.filter(diff => Array.isArray(diff.lines)) : [],
    tests: Array.isArray(payload.tests) ? payload.tests : [],
  };
}

function ensureSelectedFile(worktreeId, files) {
  if (!worktreeId || state.selectedFileByWorktree.has(worktreeId)) return;
  const firstPath = Array.isArray(files) ? files.find(file => file?.path)?.path : '';
  if (firstPath) state.selectedFileByWorktree.set(worktreeId, firstPath);
}

function fileDataKey(worktreeId, path) {
  return `${worktreeId}:${path}`;
}

function selectedFileFor(wt) {
  if (!wt?.id) return null;
  const snapshot = state.worktreeSnapshots.get(wt.id) || {};
  const files = Array.isArray(wt.files)
    ? wt.files
    : (Array.isArray(snapshot.files) ? snapshot.files : (canUseFixtures() && wt.id === WORKTREE.id ? WORKTREE.files : []));
  ensureSelectedFile(wt.id, files);
  const selectedPath = state.selectedFileByWorktree.get(wt.id);
  return files.find(file => file.path === selectedPath) || files[0] || null;
}

async function loadWorktreeFile(worktreeId, path, { force = false } = {}) {
  const worktree = findWorktreeItem(worktreeId);
  const channelId = worktree?.channel_id;
  const client = state.transport.client;
  if (!worktree?.id || !path || !channelId || !client?.connected) return;

  const key = fileDataKey(worktree.id, path);
  if (!force && (state.fileLoads.has(key) || (state.fileReads.has(key) && state.fileDiffs.has(key)))) return;
  state.fileLoads.add(key);
  state.fileErrors.delete(key);
  render();

  const snapshot = state.worktreeSnapshots.get(worktree.id) || {};
  const repoPath = snapshot.git?.repo_path || worktree.git?.repo_path || '';
  let streamedContent = '';
  try {
    const [diffResponse, readResponse] = await Promise.all([
      client.fileDiff(channelId, path, repoPath ? { repoPath } : {}),
      client.fileRead(channelId, path, {
        limit: 100000,
        onStream(frame) {
          streamedContent += frame.payload?.data || '';
        },
      }),
    ]);
    state.fileDiffs.set(key, normalizeFileDiff(diffResponse.payload));
    state.fileReads.set(key, normalizeFileRead(readResponse.payload, streamedContent));
  } catch (err) {
    state.fileErrors.set(key, String(err?.message || err));
  } finally {
    state.fileLoads.delete(key);
    render();
  }
}

function normalizeFileDiff(payload) {
  const diff = payload?.diff || '';
  return {
    path: payload?.path || '',
    repo_path: payload?.repo_path || '',
    diff,
    truncated: !!payload?.truncated,
    lines: parseUnifiedDiff(diff),
  };
}

function normalizeFileRead(payload, streamedContent = '') {
  return {
    path: payload?.path || '',
    content: payload?.content ?? streamedContent,
    size: Number(payload?.size) || 0,
    encoding: payload?.encoding || '',
    isBinary: !!payload?.is_binary,
    isImage: !!payload?.is_image,
    truncated: !!payload?.truncated,
  };
}

function reviewDiffFor(wt, path) {
  const key = fileDataKey(wt.id, path);
  const live = state.fileDiffs.get(key);
  if (live?.diff) return { diff: live.diff, source: 'live' };
  const hunk = Array.isArray(wt.diffHunks)
    ? wt.diffHunks.find(item => item.file === path)
    : null;
  const fallback = hunk || (canUseFixtures() && wt.id === WORKTREE.id ? SAMPLE_HUNKS.find(item => item.file === path) : null);
  if (!fallback?.lines?.length) return { diff: '', source: 'none' };
  const lines = [
    `--- a/${path}`,
    `+++ b/${path}`,
    '@@',
    ...fallback.lines.map(reviewDiffLine),
  ];
  return { diff: lines.join('\n'), source: 'snapshot' };
}

function reviewDiffLine(line) {
  if (line.type === 'add') return `+${line.text}`;
  if (line.type === 'del') return `-${line.text}`;
  if (line.type === 'ctx') return ` ${line.text}`;
  return line.text || '';
}

function reviewDiffPreview(diff) {
  const text = String(diff || '');
  if (text.length <= REVIEW_DIFF_PREVIEW_LIMIT) return text;
  return `${text.slice(0, REVIEW_DIFF_PREVIEW_LIMIT).trimEnd()}\n...[truncated]`;
}

function parseUnifiedDiff(diff) {
  if (!diff) return [];
  return diff.split('\n').map(line => {
    if (line.startsWith('+++') || line.startsWith('---') || line.startsWith('diff --git') || line.startsWith('index ')) {
      return { type: 'meta', text: line };
    }
    if (line.startsWith('@@')) return { type: 'hunk', text: line };
    if (line.startsWith('+')) return { type: 'add', text: line.slice(1) };
    if (line.startsWith('-')) return { type: 'del', text: line.slice(1) };
    if (line.startsWith(' ')) return { type: 'ctx', text: line.slice(1) };
    return { type: 'meta', text: line };
  });
}

function normalizeChatMessage(message) {
  if (!message?.content) return null;
  const review = parseReviewDenial(message.content);
  const interaction = normalizeInteractionData(parseMessageMetadata(message.metadata), message.id);
  const base = {
    id: message.id || '',
    author: message.sender === 'client' ? 'You' : message.sender || 'agent',
    time: relativeMessageTime(message.created_at),
    createdAt: activityTimestamp(message.created_at),
  };
  if (review) {
    return {
      ...base,
      kind: 'review_denied',
      file: review.file,
      repoPath: review.repo_path || '',
      reason: review.reason,
      diff: review.diff || '',
    };
  }
  if (interaction) {
    return {
      ...base,
      kind: 'interaction_request',
      text: message.content,
      interaction,
    };
  }
  return {
    ...base,
    text: message.content,
  };
}

function parseMessageMetadata(metadata) {
  if (metadata && typeof metadata === 'object') return metadata;
  if (typeof metadata !== 'string' || !metadata) return null;
  try {
    const parsed = JSON.parse(metadata);
    return parsed && typeof parsed === 'object' ? parsed : null;
  } catch {
    return null;
  }
}

function normalizeInteractionData(data, fallbackId = '') {
  if (!data || typeof data !== 'object') return null;
  const id = data.interaction_id || data.interactionId || data.id || fallbackId;
  if (!id) return null;
  const selectedOptions = Array.isArray(data.selected_options || data.selectedOptions)
    ? (data.selected_options || data.selectedOptions).map(String)
    : [];
  return {
    id,
    kind: data.kind || 'question',
    options: normalizeInteractionOptions(data.options),
    allowFreeform: !!(data.allowFreeform ?? data.allow_freeform),
    multiselect: !!data.multiselect,
    plan: data.plan || '',
    resolvedAt: data.resolvedAt || data.resolved_at || '',
    selectedOption: data.selectedOption || data.selected_option || '',
    selectedOptions,
    freeformResponse: data.freeformResponse || data.freeform_response || '',
  };
}

function normalizeInteractionOptions(options) {
  if (!Array.isArray(options)) return [];
  return options.map((option, index) => {
    if (option && typeof option === 'object') {
      const id = String(option.id || option.value || option.label || index);
      return { id, label: String(option.label || option.title || id) };
    }
    const id = String(option);
    return { id, label: id };
  }).filter(option => option.id).slice(0, 8);
}

function parseReviewDenial(content) {
  const text = String(content || '');
  const start = text.indexOf(REVIEW_DENIAL_BEGIN);
  const end = text.indexOf(REVIEW_DENIAL_END);
  if (start < 0 || end <= start) return null;
  const jsonText = text.slice(start + REVIEW_DENIAL_BEGIN.length, end);
  try {
    const parsed = JSON.parse(jsonText);
    if (parsed?.kind !== 'review_denied') return null;
    return parsed;
  } catch {
    return null;
  }
}

function normalizeActivityEntry(entry, channelId) {
  const type = entry?.type || entry?.kind;
  const data = entry?.data || entry?.payload || {};
  if (!type) return null;
  if (type === 'text' || type === 'thinking') {
    return {
      id: entry.id || `act-${channelId}-${entry.created_at || crypto.randomUUID()}`,
      kind: 'thinking',
      text: data.text || data.content || '',
      time: relativeMessageTime(entry.created_at),
      createdAt: activityTimestamp(entry.created_at),
    };
  }
  if (type === 'tool_use') {
    return {
      id: data.id || entry.id || `tool-${channelId}-${entry.created_at || crypto.randomUUID()}`,
      kind: 'tool_use',
      name: data.name || 'tool',
      input: data.input || {},
      time: relativeMessageTime(entry.created_at),
      createdAt: activityTimestamp(entry.created_at),
    };
  }
  if (type === 'tool_result') {
    return {
      id: data.tool_use_id || entry.id || `result-${channelId}-${entry.created_at || crypto.randomUUID()}`,
      kind: 'tool_result',
      text: stringifyActivityContent(data.content),
      isError: !!data.is_error,
      time: relativeMessageTime(entry.created_at),
      createdAt: activityTimestamp(entry.created_at),
    };
  }
  if (type === 'end') {
    return {
      id: entry.id || `end-${channelId}-${entry.created_at || crypto.randomUUID()}`,
      kind: 'activity_end',
      reason: data.reason || 'complete',
      time: relativeMessageTime(entry.created_at),
      createdAt: activityTimestamp(entry.created_at),
    };
  }
  return null;
}

function normalizeHarnesses(harnesses) {
  return Array.isArray(harnesses) ? harnesses.filter(harness => harness?.id) : [];
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
    const channelId = message.channel_id || event.detail?.target?.channel_id;
    if (channelId) {
      appendLiveMessage(channelId, normalizeChatMessage(message));
      scheduleWorktreeRefresh(channelId, { dashboard: true, delay: 900 });
    } else {
      appendLocalChat({
        author: message.sender || 'agent',
        text: message.content,
        time: 'now',
      }, chatKeyFromEvent(event.detail));
    }
    render();
  });
  client.addEventListener('v1:interaction.requested', event => {
    const payload = event.detail?.payload || {};
    const channelId = event.detail?.target?.channel_id;
    if (!channelId || !payload.interaction_id) return;
    appendLiveMessage(channelId, normalizeChatMessage({
      id: payload.interaction_id,
      channel_id: channelId,
      sender: 'agent',
      content: payload.question || 'Agent needs input',
      created_at: Date.now() / 1000,
      metadata: JSON.stringify(payload),
    }));
    scheduleWorktreeRefresh(channelId, { dashboard: true, delay: 350 });
    render();
  });
  client.addEventListener('v1:activity.delta', event => {
    const channelId = event.detail?.target?.channel_id;
    const delta = event.detail?.payload?.delta || {};
    if (!channelId || !delta.text) return;
    appendActivity(channelId, {
      id: `live-thinking-${event.detail.id}`,
      kind: 'thinking',
      text: delta.text,
      time: 'now',
      createdAt: Date.now(),
    });
    scheduleWorktreeRefresh(channelId, { delay: 1600 });
    render();
  });
  client.addEventListener('v1:tool.used', event => {
    const channelId = event.detail?.target?.channel_id;
    const payload = event.detail?.payload || {};
    if (!channelId) return;
    appendActivity(channelId, {
      id: payload.tool_use_id || `tool-${event.detail.id}`,
      kind: 'tool_use',
      name: payload.name || 'tool',
      input: payload.input || {},
      time: 'now',
      createdAt: Date.now(),
    });
    scheduleWorktreeRefresh(channelId, { delay: 1200 });
    render();
  });
  client.addEventListener('v1:tool.completed', event => {
    const channelId = event.detail?.target?.channel_id;
    const payload = event.detail?.payload || {};
    if (!channelId) return;
    appendActivity(channelId, {
      id: payload.tool_use_id || `result-${event.detail.id}`,
      kind: 'tool_result',
      text: stringifyActivityContent(payload.content),
      isError: !!payload.is_error,
      time: 'now',
      createdAt: Date.now(),
    });
    scheduleWorktreeRefresh(channelId, { dashboard: true, delay: 650 });
    render();
  });
  client.addEventListener('v1:activity.end', event => {
    const channelId = event.detail?.target?.channel_id;
    const payload = event.detail?.payload || {};
    if (!channelId) return;
    appendActivity(channelId, {
      id: `end-${event.detail.id}`,
      kind: 'activity_end',
      reason: payload.reason || 'complete',
      time: 'now',
      createdAt: Date.now(),
    });
    scheduleWorktreeRefresh(channelId, { dashboard: true, delay: 350 });
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
  } else if (action === 'respond-interaction-option') {
    respondToInteractionFromButton(actionEl);
  } else if (action === 'worktree-tab') {
    state.worktreeTab = actionEl.dataset.value || 'diff';
    render();
  } else if (action === 'select-worktree-file') {
    const worktreeId = actionEl.dataset.worktreeId;
    const path = actionEl.dataset.path;
    if (worktreeId && path) {
      state.selectedFileByWorktree.set(worktreeId, path);
      loadWorktreeFile(worktreeId, path);
      render();
    }
  } else if (action === 'refresh-worktree-file') {
    const worktreeId = actionEl.dataset.worktreeId;
    const path = actionEl.dataset.path;
    if (worktreeId && path) loadWorktreeFile(worktreeId, path, { force: true });
  } else if (action === 'open-review-deny') {
    openReviewDenyDialog(actionEl.dataset.worktreeId, actionEl.dataset.path);
  } else if (action === 'toggle-terminal') {
    state.terminalOpen = !state.terminalOpen;
    render();
  } else if (action === 'toggle-chat') {
    state.chatOpen = !state.chatOpen;
    render();
  } else if (action === 'sync-v1') {
    initTransport();
  } else if (action === 'open-project-create') {
    openProjectDialog();
  } else if (action === 'open-worktree-create') {
    openWorktreeDialog(actionEl.dataset.projectId);
  } else if (action === 'open-worktree-settings') {
    openWorktreeSettingsDialog(actionEl.dataset.worktreeId);
  } else if (action === 'stop-worktree-agent') {
    runWorktreeLifecycleAction(actionEl.dataset.worktreeId, 'stop');
  } else if (action === 'restart-worktree-agent') {
    runWorktreeLifecycleAction(actionEl.dataset.worktreeId, 'restart');
  } else if (action === 'refresh-worktree') {
    refreshWorktreeView(actionEl.dataset.worktreeId);
  } else if (action === 'refresh-project-repos') {
    loadProjectRepos(actionEl.dataset.projectId, { force: true });
  } else if (action === 'close-dialog') {
    state.dialog = null;
    render();
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
  } else if (state.dialog && target.matches('[data-dialog-field]')) {
    state.dialog.values[target.dataset.dialogField] = target.value;
  }
}

function handleChange(event) {
  const target = event.target;
  if (!state.dialog || !target.matches('[data-dialog-field]')) return;
  const field = target.dataset.dialogField;
  if (field === 'harness' || field === 'permissions' || field === 'repo_path') {
    state.dialog.values[field] = target.value;
    if (field === 'harness') {
      const harness = harnessFor(target.value);
      state.dialog.values.model = harness?.default_model || harness?.models?.[0]?.id || '';
      state.dialog.values.effort = harness?.default_effort || harness?.effort_levels?.[0] || '';
    }
    render();
  }
}

async function handleSubmit(event) {
  const form = event.target.closest('form[data-form]');
  if (!form) return;
  event.preventDefault();

  if (form.dataset.form === 'project-create') {
    await createProjectFromForm(form);
    return;
  }
  if (form.dataset.form === 'worktree-create') {
    await spawnWorktreeFromForm(form);
    return;
  }
  if (form.dataset.form === 'worktree-settings') {
    await updateWorktreeSettingsFromForm(form);
    return;
  }
  if (form.dataset.form === 'review-deny') {
    await denyReviewFromForm(form);
    return;
  }
  if (form.dataset.form === 'interaction-response') {
    await respondToInteractionFromForm(form);
    return;
  }

  const input = form.querySelector('input[name="message"], input[name="command"]');
  const value = input?.value?.trim();
  if (!value) return;
  input.value = '';

  if (form.dataset.form === 'chat') {
    const key = chatKey();
    appendLocalChat({ author: 'You', text: value, time: 'now' }, key);
    const liveChannel = firstLiveChannel();
    if (state.transport.client?.connected && liveChannel) {
      try {
        await state.transport.client.sendMessage(liveChannel.id, value);
      } catch (err) {
        appendLocalChat({ author: 'Build', text: String(err?.message || err), time: 'now' }, key);
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

function openProjectDialog() {
  state.dialog = {
    type: 'project-create',
    values: { name: '', root_path: '' },
    error: null,
    busy: false,
  };
  render();
}

function openWorktreeDialog(projectId) {
  const project = findProjectItem(projectId);
  const repos = reposForProject(projectId);
  const defaultRepo = repos[0];
  const defaults = defaultAgentOptions(project);
  const harness = harnessFor(defaults.harness) || state.harnesses.find(item => item.installed) || state.harnesses[0] || null;
  state.dialog = {
    type: 'worktree-create',
    projectId,
    values: {
      name: `Worktree ${(project.worktrees?.length || 0) + 1}`,
      repo_path: defaultRepo?.path || '',
      branch: '',
      harness: harness?.id || defaults.harness || '',
      model: defaults.model || harness?.default_model || harness?.models?.[0]?.id || '',
      effort: defaults.effort || harness?.default_effort || harness?.effort_levels?.[0] || '',
      permissions: defaults.auto_approve_tools ? 'auto' : 'ask',
    },
    error: null,
    busy: false,
  };
  render();
  loadProjectRepos(projectId);
}

function openWorktreeSettingsDialog(worktreeId) {
  const worktree = findWorktreeItem(worktreeId);
  if (!worktree?.id) return;
  const config = worktreeAgentConfig(worktree);
  const harness = harnessFor(config.harness) || state.harnesses.find(item => item.installed) || state.harnesses[0] || null;
  state.dialog = {
    type: 'worktree-settings',
    worktreeId,
    values: {
      harness: harness?.id || config.harness || '',
      model: config.model || harness?.default_model || harness?.models?.[0]?.id || '',
      effort: config.effort || harness?.default_effort || harness?.effort_levels?.[0] || '',
      permissions: config.auto_approve_tools ? 'auto' : 'ask',
    },
    error: null,
    busy: false,
  };
  render();
}

function openReviewDenyDialog(worktreeId, path) {
  const worktree = findWorktreeItem(worktreeId);
  if (!worktree?.id || !path) return;
  state.dialog = {
    type: 'review-deny',
    worktreeId,
    path,
    values: { reason: '' },
    error: null,
    busy: false,
  };
  render();
  const key = fileDataKey(worktree.id, path);
  if (!state.fileDiffs.has(key) && !state.fileLoads.has(key)) {
    loadWorktreeFile(worktree.id, path);
  }
}

async function createProjectFromForm(form) {
  const client = state.transport.client;
  if (!client?.connected) {
    setDialogError('Connect an encrypted device before creating a project.');
    return;
  }
  const name = form.elements.name?.value?.trim();
  const rootPath = form.elements.root_path?.value?.trim();
  if (!name || !rootPath) {
    setDialogError('Project name and directory are required.');
    return;
  }

  state.dialog.values = { name, root_path: rootPath };
  setDialogBusy(true);
  try {
    const response = await client.createProject({ name, root_path: rootPath });
    await refreshDashboardData();
    state.dialog = null;
    if (response.payload?.project?.id) {
      navigate(`#/project/${response.payload.project.id}`);
      loadProjectRepos(response.payload.project.id, { force: true });
    }
  } catch (err) {
    setDialogError(String(err?.message || err));
  } finally {
    setDialogBusy(false);
  }
}

async function spawnWorktreeFromForm(form) {
  const projectId = form.elements.project_id?.value || state.dialog?.projectId;
  if (!projectId || state.spawnLoads.has(projectId)) return;
  const client = state.transport.client;
  if (!client?.connected) {
    setDialogError('Connect an encrypted device before spawning a worktree.');
    return;
  }

  const project = findProjectItem(projectId);
  const harness = form.elements.harness?.value || '';
  const model = form.elements.model?.value || '';
  const effort = form.elements.effort?.value || '';
  const permissions = form.elements.permissions?.value || 'ask';
  const name = form.elements.name?.value?.trim() || `Worktree ${(project.worktrees?.length || 0) + 1}`;
  const repoPath = form.elements.repo_path?.value || '';
  const branch = form.elements.branch?.value?.trim();

  state.dialog.values = { name, repo_path: repoPath, branch, harness, model, effort, permissions };
  state.spawnLoads.add(projectId);
  setDialogBusy(true);
  render();
  try {
    const response = await client.createWorktree(projectId, {
      name,
      ...(repoPath ? { repo_path: repoPath } : {}),
      ...(branch ? { branch } : {}),
      create_git_worktree: !!repoPath,
      agent: {
        harness,
        model,
        effort,
        auto_approve_tools: permissions === 'auto',
      },
    });
    const payload = response.payload || {};
    await refreshDashboardData();
    if (payload.channel?.id && payload.agent_error) {
      appendLocalChat({ author: 'Build', text: payload.agent_error, time: 'now' }, `channel:${payload.channel.id}`);
    }
    if (payload.channel?.id && payload.git?.error) {
      appendLocalChat({ author: 'Build', text: payload.git.error, time: 'now' }, `channel:${payload.channel.id}`);
    }
    state.dialog = null;
    if (payload.worktree?.id) {
      navigate(`#/worktree/${payload.worktree.id}`);
      state.route = parseRoute();
      loadRouteData();
    }
  } catch (err) {
    setDialogError(String(err?.message || err));
  } finally {
    state.spawnLoads.delete(projectId);
    setDialogBusy(false);
    render();
  }
}

async function updateWorktreeSettingsFromForm(form) {
  const worktreeId = form.elements.worktree_id?.value || state.dialog?.worktreeId;
  const worktree = findWorktreeItem(worktreeId);
  const channelId = worktree?.channel_id;
  const client = state.transport.client;
  if (!channelId || !client?.connected) {
    setDialogError('Connect an encrypted device before updating this worktree.');
    return;
  }

  const current = worktreeAgentConfig(worktree);
  const harness = form.elements.harness?.value || '';
  const model = form.elements.model?.value || '';
  const effort = form.elements.effort?.value || '';
  const permissions = form.elements.permissions?.value || 'ask';
  const autoApprove = permissions === 'auto';
  state.dialog.values = { harness, model, effort, permissions };
  setDialogBusy(true);
  try {
    await client.updateChannel(channelId, {
      agent: {
        harness,
        model,
        effort,
        auto_approve_tools: autoApprove,
      },
    });
    const backendRestarts = harness !== current.harness || autoApprove !== current.auto_approve_tools;
    if (!backendRestarts && (model !== current.model || effort !== current.effort)) {
      await client.restartAgent(channelId);
    }
    state.dialog = null;
    await refreshWorktreeAfterAction(worktree.id, channelId);
  } catch (err) {
    setDialogError(String(err?.message || err));
  } finally {
    setDialogBusy(false);
    render();
  }
}

async function denyReviewFromForm(form) {
  const worktreeId = form.elements.worktree_id?.value || state.dialog?.worktreeId;
  const path = form.elements.path?.value || state.dialog?.path;
  const worktreeItem = findWorktreeItem(worktreeId);
  const worktree = worktreeItem ? worktreeDetail(worktreeItem) : null;
  const channelId = worktree?.channel_id;
  const client = state.transport.client;
  if (!worktree?.id || !path || !channelId || !client?.connected) {
    setDialogError('Connect an encrypted device before denying this review.');
    return;
  }

  const reason = form.elements.reason?.value?.trim() || '';
  if (!reason) {
    setDialogError('Add a denial reason so the agent knows what to change.');
    return;
  }

  const reviewDiff = reviewDiffFor(worktree, path);
  const messageId = `review_${crypto.randomUUID().replaceAll('-', '')}`;
  state.dialog.values = { reason };
  setDialogBusy(true);
  try {
    await client.denyReview(channelId, {
      message_id: messageId,
      file: path,
      reason,
      diff: reviewDiff.diff,
      repo_path: worktree.git?.repo_path || '',
    }, { id: messageId });
    appendLiveMessage(channelId, {
      id: messageId,
      kind: 'review_denied',
      author: 'You',
      file: path,
      repoPath: worktree.git?.repo_path || '',
      reason,
      diff: reviewDiff.diff,
      time: 'now',
      createdAt: Date.now(),
    });
    state.dialog = null;
    loadRouteMessages();
  } catch (err) {
    setDialogError(String(err?.message || err));
  } finally {
    setDialogBusy(false);
    render();
  }
}

async function respondToInteractionFromButton(button) {
  const channelId = button.dataset.channelId;
  const interactionId = button.dataset.interactionId;
  const optionId = button.dataset.optionId;
  const optionLabel = button.dataset.optionLabel || optionId;
  await sendInteractionResponse({
    channelId,
    interactionId,
    itemId: button.dataset.itemId,
    response: { selected_option: optionId },
    responseText: optionLabel,
  });
}

async function respondToInteractionFromForm(form) {
  const channelId = form.elements.channel_id?.value;
  const interactionId = form.elements.interaction_id?.value;
  const freeform = form.elements.freeform?.value?.trim() || '';
  if (!freeform) return;
  form.elements.freeform.value = '';
  await sendInteractionResponse({
    channelId,
    interactionId,
    itemId: form.elements.item_id?.value,
    response: { freeform },
    responseText: freeform,
  });
}

async function sendInteractionResponse({ channelId, interactionId, itemId = '', response, responseText }) {
  const client = state.transport.client;
  if (!client?.connected || !channelId || !interactionId) return;
  if (state.interactionLoads.has(interactionId)) return;
  state.interactionLoads.add(interactionId);
  state.interactionErrors.delete(interactionId);
  render();
  try {
    await client.respondToInteraction(channelId, interactionId, response);
    state.dismissedInbox.add(itemId || `interaction-${interactionId}`);
    markInteractionResolved(channelId, interactionId, response);
    appendLiveMessage(channelId, {
      id: `${interactionId}_resp`,
      author: 'You',
      text: responseText || interactionResponseText(response),
      time: 'now',
      createdAt: Date.now(),
    });
    await refreshDashboardData();
    scheduleWorktreeRefresh(channelId, { dashboard: true, delay: 350 });
  } catch (err) {
    state.interactionErrors.set(interactionId, String(err?.message || err));
  } finally {
    state.interactionLoads.delete(interactionId);
    render();
  }
}

function markInteractionResolved(channelId, interactionId, response) {
  const bucket = state.messagesByChannel.get(channelId);
  if (!bucket) return;
  for (const message of bucket) {
    if (message.kind !== 'interaction_request' || message.interaction?.id !== interactionId) continue;
    message.interaction = {
      ...message.interaction,
      resolvedAt: 'now',
      selectedOption: response.selected_option || '',
      selectedOptions: response.selected_options || [],
      freeformResponse: response.freeform || '',
    };
  }
}

function interactionResponseText(response) {
  if (response.freeform) return response.freeform;
  if (response.selected_option) return response.selected_option;
  if (Array.isArray(response.selected_options)) return response.selected_options.join(', ');
  return 'Responded';
}

async function runWorktreeLifecycleAction(worktreeId, action) {
  const worktree = findWorktreeItem(worktreeId);
  const channelId = worktree?.channel_id;
  const client = state.transport.client;
  if (!worktree?.id || !channelId || !client?.connected) return;

  const key = `${worktree.id}:${action}`;
  if (state.worktreeActionLoads.has(key)) return;
  state.worktreeActionLoads.add(key);
  state.worktreeErrors.delete(worktree.id);
  render();
  try {
    if (action === 'stop') {
      await client.stopAgent(channelId);
    } else if (action === 'restart') {
      await client.restartAgent(channelId);
    }
    await refreshWorktreeAfterAction(worktree.id, channelId);
  } catch (err) {
    state.worktreeErrors.set(worktree.id, String(err?.message || err));
  } finally {
    state.worktreeActionLoads.delete(key);
    render();
  }
}

async function refreshWorktreeAfterAction(worktreeId, channelId) {
  state.worktreeSnapshots.delete(worktreeId);
  state.activityByChannel.delete(channelId);
  state.messagesByChannel.delete(channelId);
  await Promise.all([
    loadWorktreeSnapshot(worktreeId, { force: true }),
    refreshDashboardData(),
  ]);
  loadRouteMessages();
  loadRouteActivity();
}

async function refreshWorktreeView(worktreeId) {
  const worktree = findWorktreeItem(worktreeId);
  if (!worktree?.id) return;
  state.worktreeSnapshots.delete(worktree.id);
  await Promise.all([
    loadWorktreeSnapshot(worktree.id, { force: true }),
    refreshDashboardData(),
  ]);
}

function setDialogError(message) {
  if (state.dialog) state.dialog.error = message;
  render();
}

function setDialogBusy(busy) {
  if (state.dialog) state.dialog.busy = busy;
}

async function refreshDashboardData() {
  const client = state.transport.client;
  if (!client?.connected) return;
  const [channelList, snapshot] = await Promise.all([
    client.listChannels(),
    loadDashboardSnapshot(client),
  ]);
  state.liveSnapshot = snapshot;
  setTransport({
    channels: channelList.payload?.channels || [],
    error: null,
  });
}

function defaultAgentOptions(project) {
  const worktrees = project.worktrees || [];
  const channelId = worktrees.find(worktree => worktree.channel_id)?.channel_id;
  const channel = (
    state.transport.channels.find(item => item.id === channelId)
    || state.transport.channels.find(item => item.agent?.harness)
  );
  const agent = channel?.agent || {};
  if (!agent.harness) {
    const harness = state.harnesses.find(item => item.installed) || state.harnesses[0];
    if (!harness) return {};
    return {
      harness: harness.id,
      model: harness.default_model || harness.models?.[0]?.id || '',
      effort: harness.default_effort || harness.effort_levels?.[0] || '',
      auto_approve_tools: false,
    };
  }
  return {
    harness: agent.harness,
    model: agent.model || '',
    effort: agent.effort || '',
    auto_approve_tools: !!agent.auto_approve_tools,
  };
}

function worktreeAgentConfig(worktree) {
  const snapshot = state.worktreeSnapshots.get(worktree.id) || {};
  const snapshotAgent = snapshot.agent_config || {};
  const itemAgent = worktree.agent_config || {};
  const channel = state.transport.channels.find(item => item.id === worktree.channel_id);
  const channelAgent = channel?.agent || {};
  return {
    harness: snapshotAgent.harness || itemAgent.harness || worktree.harness || channelAgent.harness || '',
    model: snapshotAgent.model || itemAgent.model || worktree.model || channelAgent.model || '',
    effort: snapshotAgent.effort || itemAgent.effort || worktree.effort || channelAgent.effort || '',
    auto_approve_tools: !!(
      snapshotAgent.auto_approve_tools
      ?? itemAgent.auto_approve_tools
      ?? worktree.auto_approve_tools
      ?? channelAgent.auto_approve_tools
    ),
    working_directory: snapshotAgent.working_directory || itemAgent.working_directory || worktree.workspace || channelAgent.working_directory || '',
    status: snapshotAgent.status || itemAgent.status || worktree.agent_status || channelAgent.status || worktree.status || '',
    is_running: !!(snapshotAgent.is_running ?? itemAgent.is_running ?? worktree.agent_running),
  };
}

function harnessFor(harnessId) {
  return state.harnesses.find(harness => harness.id === harnessId) || null;
}

function appendLocalChat(message, key = chatKey()) {
  const bucket = state.localChatByKey.get(key) || [];
  bucket.push({ createdAt: Date.now(), ...message });
  state.localChatByKey.set(key, bucket);
}

function appendLiveMessage(channelId, message) {
  if (!message) return;
  const bucket = state.messagesByChannel.get(channelId) || [];
  if (message.id && bucket.some(item => item.id === message.id)) return;
  bucket.push(message);
  state.messagesByChannel.set(channelId, bucket);
}

function appendActivity(channelId, entry) {
  const bucket = state.activityByChannel.get(channelId) || [];
  if (entry.id && bucket.some(item => item.id === entry.id)) return;
  const prev = bucket[bucket.length - 1];
  if (entry.kind === 'thinking' && prev?.kind === 'thinking' && prev.id.startsWith('live-thinking')) {
    prev.text += entry.text;
  } else {
    bucket.push(entry);
  }
  state.activityByChannel.set(channelId, bucket);
}

function localChatFor(key = chatKey()) {
  return state.localChatByKey.get(key) || [];
}

function timelineForCurrentRoute(fallback) {
  const channelId = currentRouteChannelId();
  const hasLive = channelId ? state.messagesByChannel.has(channelId) : false;
  const fallbackMessages = canUseFixtures() ? fallback : [];
  const messages = (hasLive ? state.messagesByChannel.get(channelId) || [] : fallbackMessages)
    .map(message => ({ kind: 'message', createdAt: message.createdAt || 0, ...message }));
  const activity = channelId ? state.activityByChannel.get(channelId) || [] : [];
  return [...messages, ...activity, ...localChatFor().map(message => ({ kind: 'message', ...message }))]
    .sort((a, b) => (a.createdAt || 0) - (b.createdAt || 0));
}

function chatKey() {
  const channelId = currentRouteChannelId();
  if (channelId) return `channel:${channelId}`;
  if (state.route.screen === 'plan' || state.route.screen === 'worktree') {
    return `route:${state.route.screen}:${state.route.id || ''}`;
  }
  return 'global';
}

function chatKeyFromEvent(app) {
  const channelId = app?.target?.channel_id;
  return channelId ? `channel:${channelId}` : 'global';
}

function firstLiveChannel() {
  const channelId = currentRouteChannelId();
  const routeScoped = state.route.screen === 'worktree' || state.route.screen === 'plan';
  if (channelId) {
    return state.transport.channels.find(channel => channel.id === channelId) || { id: channelId };
  }
  if (routeScoped) return null;
  return state.transport.channels[0] || null;
}

function currentRouteChannelId() {
  if (state.route.screen === 'worktree') return findWorktreeItem(state.route.id)?.channel_id || null;
  if (state.route.screen === 'plan') return findPlanItem(state.route.id)?.channel_id || null;
  return null;
}

function worktreeIdsForChannel(channelId) {
  if (!channelId) return [];
  return allWorktreesFor(projectsForRender())
    .filter(worktree => worktree.channel_id === channelId)
    .map(worktree => worktree.id)
    .filter(Boolean);
}

function scheduleWorktreeRefresh(channelId, { dashboard = false, delay = 900 } = {}) {
  const worktreeIds = worktreeIdsForChannel(channelId);
  for (const worktreeId of worktreeIds) {
    const key = `${channelId}:${worktreeId}`;
    const previous = state.snapshotRefreshTimers.get(key);
    if (previous) clearTimeout(previous);
    const timer = setTimeout(async () => {
      state.snapshotRefreshTimers.delete(key);
      state.worktreeSnapshots.delete(worktreeId);
      await loadWorktreeSnapshot(worktreeId, { force: true });
      if (dashboard) await refreshDashboardData();
    }, delay);
    state.snapshotRefreshTimers.set(key, timer);
  }
}

function canUseFixtures() {
  return state.transport.phase !== 'connected';
}

function inboxForRender() {
  return canUseFixtures() ? INBOX : (Array.isArray(state.liveSnapshot?.inbox) ? state.liveSnapshot.inbox : []);
}

function projectsForRender() {
  const projects = state.liveSnapshot?.projects;
  if (state.transport.phase === 'connected') return Array.isArray(projects) ? projects : [];
  return PROJECTS;
}

function reposForProject(projectId) {
  return state.reposByProject.get(projectId) || [];
}

function allPlansFor(projects) {
  return projects.flatMap(project => (project.plans || []).map(plan => ({ ...plan, project })));
}

function allWorktreesFor(projects) {
  return projects.flatMap(project => (project.worktrees || []).map(worktree => ({ ...worktree, project })));
}

function findProjectItem(id) {
  const projects = projectsForRender();
  return (
    projects.find(project => project.id === id)
    || (canUseFixtures() ? PROJECTS.find(project => project.id === id) : null)
    || projects[0]
    || (canUseFixtures() ? PROJECTS[0] : null)
  );
}

function findPlanItem(id) {
  const plans = allPlansFor(projectsForRender());
  const fixturePlans = canUseFixtures() ? allPlansFor(PROJECTS) : [];
  return plans.find(item => item.id === id) || fixturePlans.find(item => item.id === id) || plans[0] || fixturePlans[0] || null;
}

function findWorktreeItem(id) {
  const worktrees = allWorktreesFor(projectsForRender());
  const fixtureWorktrees = canUseFixtures() ? allWorktreesFor(PROJECTS) : [];
  return worktrees.find(item => item.id === id) || fixtureWorktrees.find(item => item.id === id) || worktrees[0] || fixtureWorktrees[0] || null;
}

function render() {
  if (!root) return;
  root.innerHTML = `
    <div class="dash-shell">
      ${renderTopChrome()}
      ${renderContent()}
      ${renderDialog()}
      <div class="status-rail">
        <span class="dot ${state.transport.phase}"></span>
        <span>${escapeHtml(state.transport.label)}</span>
      </div>
    </div>
  `;
}

function renderTopChrome() {
  const inbox = inboxForRender();
  const plansNeedingYou = inbox.filter(item => item.priority === 'high' && !state.dismissedInbox.has(item.id)).length;
  const crumbs = breadcrumb().slice(1);
  return `
    <header class="top-chrome">
      <div class="brand-block">
        <span class="brand-cube"></span>
        <span class="brand-name">build</span>
      </div>
      <nav class="nav-stack" aria-label="Dashboard">
        ${navButton('Inbox', '#/inbox', state.route.screen === 'inbox', plansNeedingYou)}
        ${navButton('Projects', '#/projects', ['projects', 'project', 'plan', 'worktree'].includes(state.route.screen), null)}
        ${crumbs.length ? `<span class="nav-slash">/</span>${crumbs.map(crumb => `
          <a class="crumb-pill" href="${escapeAttr(crumb.route || '#')}" ${crumb.route ? `data-route="${escapeAttr(crumb.route)}"` : ''}>
            <span class="dot"></span>${escapeHtml(shortCrumb(crumb.label))}
          </a>
        `).join('<span class="nav-slash">/</span>')}` : ''}
      </nav>
      <div class="top-tools">
        <label class="search-box">
          <span class="sr-only">Search</span>
          <input data-search type="search" value="${escapeAttr(state.search)}" placeholder="Search projects, plans, files...">
          <kbd>⌘K</kbd>
        </label>
        <button class="lock-pill ${state.transport.phase}" type="button" data-action="sync-v1">e2ee</button>
        <span class="user-chip">${escapeHtml(userName()[0] || 'T')}</span>
      </div>
    </header>
  `;
}

function navButton(label, route, active, count) {
  return `
    <a class="nav-button ${active ? 'active' : ''}" href="${escapeAttr(route)}" data-route="${escapeAttr(route)}">
      <span>${escapeHtml(label)}</span>
      ${count ? `<span class="nav-count">${count}</span>` : ''}
    </a>
  `;
}

function shortCrumb(label) {
  return label.length > 28 ? `${label.slice(0, 25)}...` : label;
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
    const project = findProjectItem(state.route.id);
    return [{ label: 'Projects', route: '#/projects' }, { label: project?.name || 'Missing project' }];
  }
  if (state.route.screen === 'plan') {
    const plan = findPlanItem(state.route.id);
    return [
      { label: 'Projects', route: '#/projects' },
      { label: plan?.project?.name || 'Missing project', route: plan?.project?.id ? `#/project/${plan.project.id}` : '#/projects' },
      { label: plan?.title || 'Missing plan' },
    ];
  }
  if (state.route.screen === 'worktree') {
    const worktree = findWorktreeItem(state.route.id);
    return [
      { label: 'Projects', route: '#/projects' },
      { label: worktree?.project?.name || 'Missing project', route: worktree?.project?.id ? `#/project/${worktree.project.id}` : '#/projects' },
      { label: worktree?.summary || 'Missing worktree' },
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
  if (state.route.screen === 'project') {
    const project = findProjectItem(state.route.id);
    return project ? renderProject(project) : renderMissingEntity('Project not found', 'The live project list does not contain this project.');
  }
  if (state.route.screen === 'plan') {
    const plan = findPlanItem(state.route.id);
    return plan ? renderPlan(plan) : renderMissingEntity('Plan not found', 'The live project data does not contain this plan.');
  }
  if (state.route.screen === 'worktree') {
    const worktree = findWorktreeItem(state.route.id);
    return worktree ? renderWorktree(worktree) : renderMissingEntity('Worktree not found', 'The live project data does not contain this worktree.');
  }
  return renderInbox();
}

function renderInbox() {
  const filters = ['all', 'plan-approval', 'permission', 'review', 'question'];
  const inbox = inboxForRender();
  const visible = inbox
    .filter(item => !state.dismissedInbox.has(item.id))
    .filter(item => state.inboxFilter === 'all' || item.kind === state.inboxFilter)
    .filter(matchesSearch);
  const groups = [
    ['Needs You Now', visible.filter(item => item.priority === 'high')],
    ['Ready When You Are', visible.filter(item => item.priority === 'medium' || item.kind === 'question')],
    ['Heads Up', visible.filter(item => item.priority === 'low' && item.kind !== 'question')],
  ];
  const urgentCount = inbox.filter(item => item.priority === 'high' && !state.dismissedInbox.has(item.id)).length;

  return `
    <section class="screen inbox-screen narrow-screen">
      <div class="hero-copy">
        <div class="eyebrow">Inbox</div>
        <div>
          <h1>${urgentCount} things need <em>your eyes.</em></h1>
          <p>${liveInboxSummary()}</p>
        </div>
      </div>
      <div class="screen-toolbar">
        <div class="segmented">
        ${filters.map(filter => `
          <button type="button" class="${state.inboxFilter === filter ? 'active' : ''}" data-action="inbox-filter" data-value="${escapeAttr(filter)}">
            ${escapeHtml(inboxFilterLabel(filter))}
          </button>
        `).join('')}
        </div>
        <button class="ghost-action" type="button" disabled title="Plan creation is not wired in v2 yet">+ New plan</button>
      </div>
      <div class="inbox-list">
        ${groups.map(([label, items]) => items.length ? `
          <section class="inbox-group">
            <div class="group-title"><span class="dot"></span>${escapeHtml(label)} <span>${items.length}</span></div>
            ${items.map(renderInboxItem).join('')}
          </section>
        ` : '').join('') || renderEmpty(canUseFixtures() ? 'Inbox clear' : 'No live inbox items')}
      </div>
    </section>
  `;
}

function renderInboxItem(item) {
  const project = projectsForRender().find(projectItem => projectItem.id === item.projectId) || {
    id: item.projectId || '',
    name: item.projectName || 'Project',
    color: item.projectColor || colorFor(item.projectId || item.id),
  };
  const openRoute = inboxItemRoute(item);
  const liveActions = !canUseFixtures();
  return `
    <article class="inbox-item priority-${escapeAttr(item.priority)}">
      <div class="inbox-kind">
        <span class="dot" style="--dot:${escapeAttr(project.color)}"></span>
        <span>${escapeHtml(project.name)}</span>
        <span class="kind-chip ${escapeAttr(item.kind)}">${escapeHtml(kindShortLabel(item.kind))}</span>
      </div>
      <div class="inbox-body">
        <a class="item-title" href="${escapeAttr(openRoute)}" data-route="${escapeAttr(openRoute)}">${escapeHtml(item.title)}</a>
        <p>${escapeHtml(item.detail)}</p>
      </div>
      ${agentBadge(agentKey(item.actor))}
      <time>${escapeHtml(item.time)}</time>
      <div class="item-actions">
        ${item.interaction?.id ? renderInteractionControls(item.interaction, {
          channelId: item.channelId,
          itemId: item.id,
          compact: true,
        }) : item.actions.slice(0, 2).map(action => `
          ${liveActions ? `
            <a class="secondary small" href="${escapeAttr(openRoute)}" data-route="${escapeAttr(openRoute)}">${escapeHtml(action)}</a>
          ` : `
            <button class="${action === 'Approve' || action === 'Allow' ? 'primary' : 'secondary'} small" type="button" data-action="dismiss-inbox" data-id="${escapeAttr(item.id)}">${escapeHtml(action)}</button>
          `}
        `).join('')}
      </div>
    </article>
  `;
}

function inboxItemRoute(item) {
  if (item.worktreeId) return `#/worktree/${item.worktreeId}`;
  if (item.planId) return `#/plan/${item.planId}`;
  if (item.projectId) return `#/project/${item.projectId}`;
  return '#/projects';
}

function renderInteractionControls(interaction, { channelId, itemId = '', compact = false } = {}) {
  if (!interaction?.id) return '';
  const loading = state.interactionLoads.has(interaction.id);
  const error = state.interactionErrors.get(interaction.id);
  if (interaction.resolvedAt) {
    return `<div class="interaction-resolution">Responded: ${escapeHtml(interactionResolvedLabel(interaction))}</div>`;
  }
  if (!state.transport.client?.connected || !channelId) {
    return '<div class="interaction-resolution">Open the worktree to respond</div>';
  }
  const buttons = interaction.options.map(option => `
    <button class="${interactionOptionIsPositive(option) ? 'primary' : 'secondary'} small" type="button"
      data-action="respond-interaction-option"
      data-channel-id="${escapeAttr(channelId)}"
      data-interaction-id="${escapeAttr(interaction.id)}"
      data-item-id="${escapeAttr(itemId)}"
      data-option-id="${escapeAttr(option.id)}"
      data-option-label="${escapeAttr(option.label)}"
      ${loading ? 'disabled' : ''}>
      ${escapeHtml(option.label)}
    </button>
  `).join('');
  const freeform = interaction.allowFreeform ? `
    <form class="interaction-inline ${compact ? 'compact' : ''}" data-form="interaction-response">
      <input type="hidden" name="channel_id" value="${escapeAttr(channelId)}">
      <input type="hidden" name="interaction_id" value="${escapeAttr(interaction.id)}">
      <input type="hidden" name="item_id" value="${escapeAttr(itemId)}">
      <input name="freeform" autocomplete="off" placeholder="Reply to agent" ${loading ? 'disabled' : ''}>
      <button class="secondary small" type="submit" ${loading ? 'disabled' : ''}>Send</button>
    </form>
  ` : '';
  return `
    <div class="interaction-controls ${compact ? 'compact' : ''}">
      ${buttons ? `<div class="interaction-options">${buttons}</div>` : ''}
      ${freeform}
      ${error ? `<div class="interaction-error">${escapeHtml(error)}</div>` : ''}
    </div>
  `;
}

function interactionOptionIsPositive(option) {
  const value = `${option.id} ${option.label}`.toLowerCase();
  return value.includes('approve') || value.includes('accept') || value.includes('allow');
}

function interactionResolvedLabel(interaction) {
  if (interaction.freeformResponse) return interaction.freeformResponse;
  if (interaction.selectedOptions?.length) {
    return interaction.selectedOptions.map(id => interactionOptionLabel(interaction, id)).join(', ');
  }
  if (interaction.selectedOption) return interactionOptionLabel(interaction, interaction.selectedOption);
  return 'done';
}

function interactionOptionLabel(interaction, optionId) {
  return interaction.options.find(option => option.id === optionId)?.label || optionId;
}

function inboxFilterLabel(filter) {
  const inbox = inboxForRender();
  if (filter === 'all') return `All ${inbox.length}`;
  const count = inbox.filter(item => item.kind === filter).length;
  const label = {
    'plan-approval': 'Plans',
    permission: 'Permissions',
    review: 'Reviews',
    question: 'Questions',
  }[filter] || labelFor(filter);
  return `${label} ${count}`;
}

function liveInboxSummary() {
  const projects = projectsForRender();
  const agents = sum(projects, 'runningAgents');
  if (canUseFixtures()) return `${agents} agents working across ${projects.length} projects. Most won't need you.`;
  const inbox = inboxForRender();
  if (inbox.length) return `${inbox.length} live item${inbox.length === 1 ? '' : 's'} need review across ${projects.length} projects.`;
  return `${agents} live agents across ${projects.length} projects. No pending inbox items.`;
}

function kindShortLabel(kind) {
  return {
    'plan-approval': 'plan',
    permission: 'permission',
    review: 'review',
    question: 'question',
  }[kind] || kind;
}

function renderProjects() {
  const filters = ['all', 'needs-you', 'running', 'queued'];
  const projects = projectsForRender()
    .filter(project => {
      if (state.projectFilter === 'needs-you') return project.needsYou > 0;
      if (state.projectFilter === 'running') return project.runningAgents > 0;
      if (state.projectFilter === 'queued') return project.queued > 0;
      return true;
    })
    .filter(matchesSearch);

  return `
    <section class="screen projects-screen narrow-screen">
      <div class="screen-head">
        <div>
          <div class="eyebrow">Projects</div>
          <h1>Your <em>workshops.</em></h1>
        </div>
        <button class="primary" type="button" data-action="open-project-create">+ New project</button>
      </div>
      <div class="project-grid">
        ${projects.map(project => `
          <article class="project-card" style="--project:${escapeAttr(project.color)}" data-route="#/project/${escapeAttr(project.id)}">
            <a class="card-hit" href="#/project/${escapeAttr(project.id)}" data-route="#/project/${escapeAttr(project.id)}" aria-label="${escapeAttr(project.name)}"></a>
            <div class="project-top">
              <div>
                <h2><span class="project-color" style="--project:${escapeAttr(project.color)}"></span>${escapeHtml(project.name)}</h2>
                <p>${escapeHtml(project.description)}</p>
              </div>
              ${project.needsYou ? `<span class="need-chip">${project.needsYou} need you</span>` : ''}
            </div>
            <div class="project-stats">
              ${compactMetric('Agents', project.runningAgents)}
              ${compactMetric('Queued', project.queued)}
              ${compactMetric('Plans', project.plans.length)}
            </div>
            <div class="mini-list">
              ${project.worktrees.length ? project.worktrees.slice(0, 2).map(wt => `
                <div>
                  ${agentBadge(wt.model)}
                  <span class="truncate">${escapeHtml(wt.branch.replace('agents/', 'agents/'))}</span>
                  <span class="mini-status ${escapeAttr(wt.status)}">${escapeHtml(wt.status)}</span>
                </div>
              `).join('') : `<p class="muted-line">No agents running . last active ${escapeHtml(project.lastActive)}</p>`}
            </div>
          </article>
        `).join('') || renderEmpty(canUseFixtures() ? 'No projects' : 'No live projects yet. Create a project to start.')}
      </div>
    </section>
  `;
}

function compactMetric(label, value) {
  return `
    <div class="compact-metric">
      <span>${escapeHtml(String(value))}</span>
      <span>${escapeHtml(label)}</span>
    </div>
  `;
}

function renderProject(project) {
  const lanes = [
    ['in-progress', 'In Progress'],
    ['queued', 'Queued'],
    ['draft', 'Drafts'],
  ];
  const spawning = state.spawnLoads.has(project.id);
  return `
    <section class="screen project-screen">
      <div class="project-hero plain-hero" style="--project:${escapeAttr(project.color)}">
        <div>
          <h1><span class="project-color" style="--project:${escapeAttr(project.color)}"></span>${escapeHtml(project.name)}</h1>
          <p>${escapeHtml(project.description)}</p>
        </div>
        <div class="workspace-actions">
          <button class="ghost-action" type="button" data-action="sync-v1">Sync</button>
          <button class="primary" type="button" disabled title="Plan creation is not wired in v2 yet">+ New plan</button>
        </div>
      </div>
      <div class="project-layout">
        <section class="plan-board">
          <div class="panel-title-row">
            <div class="panel-title">Plan board</div>
            <span class="muted-line">${canUseFixtures() ? 'Drag a plan onto a worktree to assign . click to iterate' : 'Live plans from project primitives'}</span>
          </div>
          ${lanes.map(([status, label]) => `
            <div class="plan-lane">
              <div class="lane-title"><span class="dot"></span>${escapeHtml(label)} <span>${project.plans.filter(plan => plan.status === status).length}</span></div>
              ${project.plans.filter(plan => plan.status === status).map(plan => renderPlanCard(plan, project)).join('') || ''}
            </div>
          `).join('')}
        </section>
        <aside class="project-aside">
          ${renderProjectRepos(project)}
          <div class="panel worktree-panel">
            <div class="panel-title-row">
              <div class="panel-title">Worktrees . ${project.worktrees.length}</div>
              <span class="muted-line">${canUseFixtures() ? 'Drop a plan on one to queue work' : 'Spawn agents against discovered repos'}</span>
            </div>
            ${project.worktrees.map(wt => `
              <a class="worktree-card" href="#/worktree/${escapeAttr(wt.id)}" data-route="#/worktree/${escapeAttr(wt.id)}">
                <div class="worktree-card-top">
                  ${agentBadge(wt.model)}
                  <code>${escapeHtml(wt.branch)}</code>
                  <span class="mini-status ${escapeAttr(wt.status)}">${escapeHtml(wt.status)}</span>
                </div>
                <span class="worktree-main">
                  <span>${escapeHtml(wt.summary)}</span>
                  <span>${escapeHtml(wt.device)} . +${wt.add} -${wt.del} . ${wt.files}f</span>
                </span>
                <span class="progress"><span style="width:${wt.pct}%"></span></span>
              </a>
            `).join('') || '<div class="repo-empty">No live worktrees</div>'}
            <button class="secondary full" type="button" data-action="open-worktree-create" data-project-id="${escapeAttr(project.id)}" ${spawning ? 'disabled' : ''}>
              ${spawning ? 'Spawning...' : '+ Spawn new worktree'}
            </button>
          </div>
          ${canUseFixtures() || project.activity.length ? `<div class="panel">
            <div class="panel-title">Recent</div>
            <div class="activity-list">
              ${project.activity.map(item => `<div>${escapeHtml(item)}</div>`).join('')}
            </div>
          </div>` : ''}
        </aside>
      </div>
    </section>
  `;
}

function renderProjectRepos(project) {
  const repos = reposForProject(project.id);
  const loading = state.repoLoads.has(project.id);
  const error = state.repoErrors.get(project.id);
  return `
    <div class="panel repo-panel">
      <div class="panel-title-row">
        <div class="panel-title">Repositories . ${repos.length}</div>
        <button class="text-button" type="button" data-action="refresh-project-repos" data-project-id="${escapeAttr(project.id)}" ${loading ? 'disabled' : ''}>
          ${loading ? 'Scanning' : 'Refresh'}
        </button>
      </div>
      <p class="repo-root">${escapeHtml(project.root_path || project.repo || project.name)}</p>
      ${error ? `<div class="form-error">${escapeHtml(error)}</div>` : ''}
      <div class="repo-list">
        ${repos.map(repo => `
          <div class="repo-row">
            <div>
              <strong>${escapeHtml(repo.name)}</strong>
              <span>${escapeHtml(repo.relative_path || repo.path)}</span>
            </div>
            <code>${escapeHtml(repo.branch)}</code>
          </div>
        `).join('') || `<div class="repo-empty">${loading ? 'Scanning project directory...' : 'No git repositories discovered'}</div>`}
      </div>
    </div>
  `;
}

function renderPlanCard(plan, project) {
  const pct = Math.round((plan.doneSteps / Math.max(plan.steps, 1)) * 100);
  return `
    <article class="plan-card plan-row" data-route="#/plan/${escapeAttr(plan.id)}">
      <a class="card-hit" href="#/plan/${escapeAttr(plan.id)}" data-route="#/plan/${escapeAttr(plan.id)}" aria-label="${escapeAttr(plan.title)}"></a>
      <code>${escapeHtml(plan.id)}</code>
      <h3>${escapeHtml(plan.title)}</h3>
      ${agentBadge(plan.model)}
      <div class="progress"><span style="width:${pct}%"></span></div>
      <div class="plan-meta">
        <span>${plan.doneSteps ? `${plan.doneSteps}/${plan.steps}` : `${plan.steps} steps`}</span>
        <span>${escapeHtml(plan.updated)}</span>
      </div>
    </article>
  `;
}

function renderPlan(planItem) {
  const project = planItem.project;
  const useFixturePlan = canUseFixtures() && planItem.id === PLAN_DOC.id;
  const doc = useFixturePlan ? PLAN_DOC : { id: planItem.id, title: planItem.title, chat: [] };
  const messages = timelineForCurrentRoute(doc.chat || []);
  return `
    <section class="plan-screen">
      <div class="subbar">
        <div><strong>Plan</strong> ${escapeHtml(doc.id)} <span class="dot"></span> ${escapeHtml(planItem.status || 'draft')} ${planItem.updated ? `. updated ${escapeHtml(planItem.updated)}` : ''}</div>
        <div class="workspace-actions">
          <button class="ghost-action" type="button" disabled title="Plan versions are not wired in v2 yet">Versions</button>
          <button class="primary small" type="button" disabled title="Plan assignment is not wired in v2 yet">Assign</button>
        </div>
      </div>
      <div class="plan-layout">
        ${useFixturePlan ? renderFixturePlanDocument(doc) : renderLivePlanDocument(planItem)}
        <aside class="chat-panel ${state.chatOpen ? '' : 'collapsed'}">
          <div class="panel-title-row">
            <div class="panel-title">Conversation <span class="muted-line">${planItem.channel_id ? 'live channel' : 'no channel'}</span></div>
            <button class="icon-button" type="button" data-action="toggle-chat" aria-label="Toggle chat">C</button>
          </div>
          ${state.chatOpen ? renderChat(messages, 'plan') : ''}
        </aside>
      </div>
    </section>
  `;
}

function renderFixturePlanDocument(doc) {
  return `
    <article class="document-panel">
      <h1>${escapeHtml(doc.title)}</h1>
      <div class="goal-box">
        <div class="eyebrow">Goal</div>
        <p>Make every request through api-gateway carry a tenant context, and prevent any cross-tenant data leakage. Ship behind a flag.</p>
      </div>
      ${doc.phases.map((phase, index) => `
        <section class="doc-section">
          <button class="phase-caret" type="button">⌄</button>
          <div class="section-number">${index + 1}</div>
          <div class="phase-body">
            <h2>${escapeHtml(phase.title)}</h2>
            <span class="phase-state ${index === 0 ? 'done' : index === 1 ? 'active' : 'approval'}">${index === 0 ? 'done' : index === 1 ? 'active' : 'needs approval'}</span>
            ${index === 2 ? `<div class="approval-row"><span>Need approval - 6 tables affected</span><button class="secondary small" type="button">Approve phase</button><button class="ghost-action" type="button">Discuss</button></div>` : ''}
            <ul class="check-list">
              ${phase.steps.map((step, stepIndex) => `<li class="${index === 0 || stepIndex === 0 ? 'checked' : ''}">${escapeHtml(step)}</li>`).join('')}
            </ul>
          </div>
        </section>
      `).join('')}
    </article>
  `;
}

function renderLivePlanDocument(plan) {
  const pct = Math.round((Number(plan.doneSteps) / Math.max(Number(plan.steps) || 1, 1)) * 100);
  return `
    <article class="document-panel">
      <h1>${escapeHtml(plan.title)}</h1>
      <div class="goal-box">
        <div class="eyebrow">Live plan primitive</div>
        <p>${escapeHtml(plan.status || 'draft')} . ${escapeHtml(String(plan.doneSteps || 0))}/${escapeHtml(String(plan.steps || 1))} steps . ${escapeHtml(plan.model || 'default')}</p>
      </div>
      <section class="doc-section">
        <div class="section-number">1</div>
        <div class="phase-body">
          <h2>Current status</h2>
          <span class="phase-state active">${escapeHtml(plan.status || 'draft')}</span>
          <div class="progress"><span style="width:${pct}%"></span></div>
          <p class="muted-line">Detailed plan body editing is not wired in v2 yet.</p>
        </div>
      </section>
    </article>
  `;
}

function renderWorktree(worktreeItem) {
  const project = worktreeItem.project;
  const wt = worktreeDetail(worktreeItem);
  const config = wt.agent_config || worktreeAgentConfig(wt);
  const channelId = wt.channel_id;
  const stopping = state.worktreeActionLoads.has(`${wt.id}:stop`);
  const restarting = state.worktreeActionLoads.has(`${wt.id}:restart`);
  const worktreeError = state.worktreeErrors.get(wt.id);
  const selectedFile = selectedFileFor(wt);
  const messages = timelineForCurrentRoute(wt.chat || (canUseFixtures() ? WORKTREE.chat : []));
  const loadingSnapshot = state.worktreeLoads.has(wt.id);
  const tabs = ['diff', 'files', 'git', 'tests'];
  return `
    <section class="worktree-screen">
      <div class="subbar worktree-subbar">
        <div><strong>${escapeHtml(wt.branch)}</strong> <span>${escapeHtml(wt.git?.branch || project.branch || 'main')}</span> <span class="mini-status ${escapeAttr(wt.status)}">${escapeHtml(wt.status)}</span> <span>${escapeHtml(wt.updated || project.lastActive || 'now')}</span></div>
        <div class="workspace-actions">
          ${agentBadge(wt.model || wt.agent)}
          <span>${escapeHtml(config.model || wt.agent)} . ${escapeHtml(config.status || wt.status)}</span>
          <button class="ghost-action" type="button" data-action="refresh-worktree" data-worktree-id="${escapeAttr(wt.id)}">Refresh</button>
          <button class="secondary small" type="button" data-action="open-worktree-settings" data-worktree-id="${escapeAttr(wt.id)}" ${!channelId ? 'disabled' : ''}>Settings</button>
          <button class="secondary small" type="button" data-action="stop-worktree-agent" data-worktree-id="${escapeAttr(wt.id)}" ${!channelId || stopping ? 'disabled' : ''}>${stopping ? 'Stopping...' : 'Stop'}</button>
          <button class="primary small" type="button" data-action="restart-worktree-agent" data-worktree-id="${escapeAttr(wt.id)}" ${!channelId || restarting ? 'disabled' : ''}>${restarting ? 'Restarting...' : 'Restart'}</button>
        </div>
      </div>
      <div class="worktree-meta-strip">
        ${metaPill('Repo', wt.git?.repo_path || project.repo || project.name)}
        ${metaPill('Path', wt.workspace || config.working_directory || '')}
        ${metaPill('Model', config.model || wt.model || 'default')}
        ${metaPill('Thinking', config.effort || 'default')}
        ${metaPill('Permissions', config.auto_approve_tools ? 'auto' : 'ask')}
        ${metaPill('Agent', config.is_running ? 'running' : config.status || wt.status)}
      </div>
      ${worktreeError ? `<div class="worktree-error">${escapeHtml(worktreeError)}</div>` : ''}
      ${wt.git?.error ? `<div class="worktree-error">${escapeHtml(wt.git.error)}</div>` : ''}
      <div class="worktree-grid">
        <aside class="file-rail">
          <div class="rail-tabs">
            <button class="active" type="button">Changed <span>${wt.files.length}</span></button>
            <button type="button">All</button>
          </div>
          <div class="rail-summary"><span class="add">+${sum(wt.files, 'add')}</span><span class="del">-${sum(wt.files, 'del')}</span><span>${wt.files.length} changed</span></div>
          ${wt.files.map(file => `
            <button class="changed-file ${selectedFile?.path === file.path ? 'active' : ''}" type="button" data-action="select-worktree-file" data-worktree-id="${escapeAttr(wt.id)}" data-path="${escapeAttr(file.path)}">
              <span>${escapeHtml((file.status || 'M')[0].toUpperCase())}</span>
              <span>${escapeHtml(file.path)}</span>
              <span class="add">+${file.add}</span>
              <span class="del">-${file.del}</span>
            </button>
          `).join('') || `<div class="repo-empty">${loadingSnapshot ? 'Loading live changes...' : 'No changed files'}</div>`}
        </aside>
        <section class="workbench">
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
        </section>
        <aside class="chat-panel worktree-chat ${state.chatOpen ? '' : 'collapsed'}">
          <div class="panel-title-row">
            <div class="panel-title">${escapeHtml(wt.agent)} <span class="mini-status ${escapeAttr(wt.status)}">${escapeHtml(wt.status)}</span></div>
            <button class="icon-button" type="button" data-action="toggle-chat" aria-label="Toggle chat">C</button>
          </div>
          ${state.chatOpen ? renderChat(messages, 'worktree') : ''}
        </aside>
      </div>
      <section class="terminal-panel docked-terminal ${state.terminalOpen ? '' : 'collapsed'}">
        <div class="panel-title-row">
          <div class="panel-title">Terminal <span class="muted-line">${escapeHtml(wt.device)} . ${escapeHtml(wt.branch)}</span></div>
          <button class="icon-button" type="button" data-action="toggle-terminal" aria-label="Toggle terminal">x</button>
        </div>
        ${state.terminalOpen ? renderTerminal() : ''}
      </section>
    </section>
  `;
}

function worktreeDetail(worktreeItem) {
  const snapshot = state.worktreeSnapshots.get(worktreeItem.id) || {};
  const useFixtureWorktree = canUseFixtures() && worktreeItem.id === WORKTREE.id;
  const detail = useFixtureWorktree ? { ...WORKTREE, ...snapshot } : { ...worktreeItem, ...snapshot };
  const agentConfig = worktreeAgentConfig({ ...worktreeItem, ...snapshot });
  detail.agent_config = agentConfig;
  detail.harness = agentConfig.harness;
  detail.effort = agentConfig.effort;
  detail.auto_approve_tools = agentConfig.auto_approve_tools;
  detail.agent_running = agentConfig.is_running;
  detail.agent_status = agentConfig.status;
  if (agentConfig.model) detail.model = agentConfig.model;
  if (!Array.isArray(detail.files)) detail.files = useFixtureWorktree ? WORKTREE.files : [];
  if (!detail.git) detail.git = useFixtureWorktree ? WORKTREE.git : emptyGit(detail);
  if (!Array.isArray(detail.tests)) detail.tests = useFixtureWorktree ? WORKTREE.tests : [];
  if (!Array.isArray(detail.chat)) detail.chat = useFixtureWorktree ? WORKTREE.chat : [];
  if (!Array.isArray(detail.terminal)) detail.terminal = useFixtureWorktree ? WORKTREE.terminal : [];
  if (!detail.agent) detail.agent = detail.model || 'device';
  if (!detail.device) detail.device = 'local';
  return detail;
}

function emptyGit(worktree) {
  return {
    repo_path: '',
    branch: worktree.branch || 'workspace',
    staged: 0,
    unstaged: 0,
    commits: [],
    error: null,
  };
}

function renderWorktreeTab(wt) {
  if (state.worktreeTab === 'files') return renderFiles(wt);
  if (state.worktreeTab === 'git') return renderGit(wt);
  if (state.worktreeTab === 'tests') return renderTests(wt);
  return renderDiff(wt);
}

function renderDiff(wt) {
  const selected = selectedFileFor(wt);
  if (!selected) return '<div class="empty-state">No changed file selected</div>';
  const key = fileDataKey(wt.id, selected.path);
  const live = state.fileDiffs.get(key);
  const loading = state.fileLoads.has(key);
  const error = state.fileErrors.get(key);
  const snapshotHunk = Array.isArray(wt.diffHunks)
    ? wt.diffHunks.find(hunk => hunk.file === selected.path)
    : null;
  const fallbackHunks = snapshotHunk ? [snapshotHunk] : (canUseFixtures() && wt.id === WORKTREE.id ? SAMPLE_HUNKS : []);
  const hunks = live?.lines?.length
    ? [{ file: live.path || selected.path, add: selected.add, del: selected.del, lines: live.lines, truncated: live.truncated }]
    : fallbackHunks;
  return `
    <div class="diff-view">
      <div class="review-toolbar">
        <div>
          <strong>${escapeHtml(selected.path)}</strong>
          <span>${escapeHtml(selected.status || 'M')} . +${selected.add} -${selected.del}</span>
        </div>
        <div class="review-actions">
          <button class="ghost-action small" type="button" data-action="refresh-worktree-file" data-worktree-id="${escapeAttr(wt.id)}" data-path="${escapeAttr(selected.path)}" ${loading ? 'disabled' : ''}>${loading ? 'Loading...' : 'Refresh file'}</button>
          <button class="secondary small" type="button" disabled title="Approve is not exposed in v1 yet">Approve</button>
          <button class="secondary small danger-action" type="button" data-action="open-review-deny" data-worktree-id="${escapeAttr(wt.id)}" data-path="${escapeAttr(selected.path)}" ${wt.channel_id ? '' : 'disabled'}>Deny</button>
        </div>
      </div>
      ${error ? `<div class="worktree-error">${escapeHtml(error)}</div>` : ''}
      ${hunks.map(hunk => `
        <section class="diff-file">
          <div class="diff-file-head">
            <span class="kind-chip permission">${live ? 'live' : 'snapshot'}</span>
            <code>${escapeHtml(hunk.file)}</code>
            <span class="add">+${hunk.add ?? countLines(hunk, 'add')}</span>
            <span class="del">-${hunk.del ?? countLines(hunk, 'del')}</span>
          </div>
          <div class="hunk-head">
            <span>${live ? 'live diff' : 'snapshot diff'}</span>
            <span>${live?.truncated || hunk.truncated ? 'truncated' : 'worktree changes'}</span>
          </div>
          <pre>${hunk.lines.map(line => `<span class="${escapeAttr(line.type)}">${escapeHtml(prefixFor(line.type) + line.text)}</span>`).join('')}</pre>
        </section>
      `).join('') || `<div class="empty-state">${loading ? 'Loading live diff...' : 'No diff available for this file'}</div>`}
    </div>
  `;
}

function countLines(hunk, type) {
  return Array.isArray(hunk.lines) ? hunk.lines.filter(line => line.type === type).length : 0;
}

function renderFiles(wt) {
  const selected = selectedFileFor(wt);
  const key = selected ? fileDataKey(wt.id, selected.path) : '';
  const read = key ? state.fileReads.get(key) : null;
  const loading = key ? state.fileLoads.has(key) : false;
  const error = key ? state.fileErrors.get(key) : '';
  return `
    <div class="file-layout">
      <div class="tree-panel">
        ${(canUseFixtures() && wt.id === WORKTREE.id ? FILE_TREE : wt.files.map(file => ({ type: 'file', path: file.path, depth: 0, changed: true }))).map(node => `
          <div class="tree-row ${node.changed ? 'changed' : ''}" style="--depth:${node.depth}">
            <span>${node.type === 'dir' ? 'dir' : 'file'}</span>
            <span>${escapeHtml(node.path.split('/').pop())}</span>
          </div>
        `).join('') || '<div class="empty-state">No live file tree yet</div>'}
      </div>
      <div class="file-list-panel">
        ${wt.files.map(file => `
          <div class="file-row">
            <span>${escapeHtml(file.path)}</span>
            <span>${escapeHtml(file.status)}</span>
            <span class="add">+${file.add}</span>
            <span class="del">-${file.del}</span>
          </div>
        `).join('') || '<div class="empty-state">No changed files</div>'}
      </div>
      <div class="file-read-panel">
        <div class="file-read-head">
          <strong>${escapeHtml(selected?.path || 'No file selected')}</strong>
          ${selected ? `<button class="ghost-action small" type="button" data-action="refresh-worktree-file" data-worktree-id="${escapeAttr(wt.id)}" data-path="${escapeAttr(selected.path)}" ${loading ? 'disabled' : ''}>${loading ? 'Loading...' : 'Refresh'}</button>` : ''}
        </div>
        ${error ? `<div class="worktree-error">${escapeHtml(error)}</div>` : ''}
        ${renderFileReadBody(read, loading)}
      </div>
    </div>
  `;
}

function renderFileReadBody(read, loading) {
  if (loading && !read) return '<div class="empty-state">Loading file...</div>';
  if (!read) return '<div class="empty-state">Select a changed file to read it</div>';
  if (read.isBinary) return `<div class="empty-state">${read.isImage ? 'Image preview is not wired here yet' : 'Binary file'} . ${read.size} bytes</div>`;
  return `
    <pre class="file-read-output">${escapeHtml(read.content || '')}</pre>
    <div class="file-read-foot">${read.truncated ? 'Truncated at 100 KB' : `${read.size} bytes`}</div>
  `;
}

function renderGit(wt) {
  const commits = Array.isArray(wt.git?.commits) ? wt.git.commits : [];
  return `
    <div class="git-panel">
      <div class="git-summary">
        ${metric('Staged', wt.git?.staged || 0)}
        ${metric('Unstaged', wt.git?.unstaged || 0)}
      </div>
      ${commits.map(commit => `
        <div class="commit-row">
          <code>${escapeHtml(commit.sha)}</code>
          <span>${escapeHtml(commit.message)}</span>
          <span>${escapeHtml(commit.time)}</span>
        </div>
      `).join('') || '<div class="empty-state">No live commits loaded</div>'}
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
      `).join('') || '<div class="empty-state">No live test results</div>'}
    </div>
  `;
}

function renderDialog() {
  if (!state.dialog) return '';
  if (state.dialog.type === 'project-create') return renderProjectDialog();
  if (state.dialog.type === 'worktree-create') return renderWorktreeDialog();
  if (state.dialog.type === 'worktree-settings') return renderWorktreeSettingsDialog();
  if (state.dialog.type === 'review-deny') return renderReviewDenyDialog();
  return '';
}

function renderProjectDialog() {
  const dialog = state.dialog;
  return `
    <div class="modal-backdrop" role="dialog" aria-modal="true">
      <form class="modal-panel" data-form="project-create">
        <header class="modal-head">
          <h2>Create project</h2>
          <button class="icon-button" type="button" data-action="close-dialog" aria-label="Close">x</button>
        </header>
        <label class="form-field">
          <span>Name</span>
          <input name="name" autocomplete="off" placeholder="Build web" value="${escapeAttr(dialog.values.name)}">
        </label>
        <label class="form-field">
          <span>Directory</span>
          <input name="root_path" autocomplete="off" spellcheck="false" placeholder="/Users/you/Projects/repo-or-workspace" value="${escapeAttr(dialog.values.root_path)}">
        </label>
        ${dialog.error ? `<div class="form-error">${escapeHtml(dialog.error)}</div>` : ''}
        <footer class="modal-actions">
          <button class="secondary" type="button" data-action="close-dialog">Cancel</button>
          <button class="primary" type="submit" ${dialog.busy ? 'disabled' : ''}>${dialog.busy ? 'Creating...' : 'Create project'}</button>
        </footer>
      </form>
    </div>
  `;
}

function renderWorktreeDialog() {
  const dialog = state.dialog;
  const project = findProjectItem(dialog.projectId);
  const repos = reposForProject(project.id);
  const reposLoading = state.repoLoads.has(project.id);
  const repoError = state.repoErrors.get(project.id);
  const harness = harnessFor(dialog.values.harness) || state.harnesses[0] || {};
  const models = harness.models || [];
  const efforts = harness.effort_levels || ['low', 'medium', 'high', 'xhigh'];
  return `
    <div class="modal-backdrop" role="dialog" aria-modal="true">
      <form class="modal-panel" data-form="worktree-create">
        <input type="hidden" name="project_id" value="${escapeAttr(project.id)}">
        <header class="modal-head">
          <h2>Spawn worktree</h2>
          <button class="icon-button" type="button" data-action="close-dialog" aria-label="Close">x</button>
        </header>
        <label class="form-field">
          <span>Name</span>
          <input name="name" autocomplete="off" value="${escapeAttr(dialog.values.name)}">
        </label>
        <label class="form-field">
          <span>Repository</span>
          <select name="repo_path" data-dialog-field="repo_path" ${reposLoading || !repos.length ? 'disabled' : ''}>
            ${repos.map(repo => `
              <option value="${escapeAttr(repo.path)}" ${repo.path === dialog.values.repo_path ? 'selected' : ''}>
                ${escapeHtml(repo.relative_path || repo.name)} . ${escapeHtml(repo.branch)}
              </option>
            `).join('') || `<option value="">${reposLoading ? 'Scanning repositories...' : 'No git repositories found'}</option>`}
          </select>
        </label>
        <label class="form-field">
          <span>Branch</span>
          <input name="branch" autocomplete="off" spellcheck="false" placeholder="agents/my-task" value="${escapeAttr(dialog.values.branch)}">
        </label>
        <div class="form-grid">
          <label class="form-field">
            <span>Model provider</span>
            <select name="harness" data-dialog-field="harness">
              ${state.harnesses.map(item => `<option value="${escapeAttr(item.id)}" ${item.id === dialog.values.harness ? 'selected' : ''}>${escapeHtml(item.name || item.id)}${item.installed ? '' : ' (missing)'}</option>`).join('') || '<option value="">No harnesses</option>'}
            </select>
          </label>
          <label class="form-field">
            <span>Model</span>
            <select name="model">
              ${models.map(model => `<option value="${escapeAttr(model.id)}" ${model.id === dialog.values.model ? 'selected' : ''}>${escapeHtml(model.name || model.id)}</option>`).join('') || `<option value="${escapeAttr(dialog.values.model)}">${escapeHtml(dialog.values.model || 'Default')}</option>`}
            </select>
          </label>
          <label class="form-field">
            <span>Thinking</span>
            <select name="effort">
              ${efforts.map(effort => `<option value="${escapeAttr(effort)}" ${effort === dialog.values.effort ? 'selected' : ''}>${escapeHtml(effort)}</option>`).join('')}
            </select>
          </label>
          <label class="form-field">
            <span>Permissions</span>
            <select name="permissions" data-dialog-field="permissions">
              <option value="ask" ${dialog.values.permissions !== 'auto' ? 'selected' : ''}>Ask before tools</option>
              <option value="auto" ${dialog.values.permissions === 'auto' ? 'selected' : ''}>Auto-approve tools</option>
            </select>
          </label>
        </div>
        <p class="form-hint">${escapeHtml(project.root_path || project.repo || project.name)}</p>
        ${repoError ? `<div class="form-error">${escapeHtml(repoError)}</div>` : ''}
        ${dialog.error ? `<div class="form-error">${escapeHtml(dialog.error)}</div>` : ''}
        <footer class="modal-actions">
          <button class="secondary" type="button" data-action="close-dialog">Cancel</button>
          <button class="primary" type="submit" ${dialog.busy || reposLoading || !repos.length ? 'disabled' : ''}>${dialog.busy ? 'Spawning...' : 'Spawn agent'}</button>
        </footer>
      </form>
    </div>
  `;
}

function renderWorktreeSettingsDialog() {
  const dialog = state.dialog;
  const worktree = findWorktreeItem(dialog.worktreeId);
  const harness = harnessFor(dialog.values.harness) || state.harnesses[0] || {};
  const models = harness.models || [];
  const efforts = harness.effort_levels || ['low', 'medium', 'high', 'xhigh'];
  return `
    <div class="modal-backdrop" role="dialog" aria-modal="true">
      <form class="modal-panel" data-form="worktree-settings">
        <input type="hidden" name="worktree_id" value="${escapeAttr(worktree.id)}">
        <header class="modal-head">
          <h2>Agent settings</h2>
          <button class="icon-button" type="button" data-action="close-dialog" aria-label="Close">x</button>
        </header>
        <div class="form-grid">
          <label class="form-field">
            <span>Model provider</span>
            <select name="harness" data-dialog-field="harness">
              ${state.harnesses.map(item => `<option value="${escapeAttr(item.id)}" ${item.id === dialog.values.harness ? 'selected' : ''}>${escapeHtml(item.name || item.id)}${item.installed ? '' : ' (missing)'}</option>`).join('') || '<option value="">No harnesses</option>'}
            </select>
          </label>
          <label class="form-field">
            <span>Model</span>
            <select name="model">
              ${models.map(model => `<option value="${escapeAttr(model.id)}" ${model.id === dialog.values.model ? 'selected' : ''}>${escapeHtml(model.name || model.id)}</option>`).join('') || `<option value="${escapeAttr(dialog.values.model)}">${escapeHtml(dialog.values.model || 'Default')}</option>`}
            </select>
          </label>
          <label class="form-field">
            <span>Thinking</span>
            <select name="effort">
              ${efforts.map(effort => `<option value="${escapeAttr(effort)}" ${effort === dialog.values.effort ? 'selected' : ''}>${escapeHtml(effort)}</option>`).join('')}
            </select>
          </label>
          <label class="form-field">
            <span>Permissions</span>
            <select name="permissions" data-dialog-field="permissions">
              <option value="ask" ${dialog.values.permissions !== 'auto' ? 'selected' : ''}>Ask before tools</option>
              <option value="auto" ${dialog.values.permissions === 'auto' ? 'selected' : ''}>Auto-approve tools</option>
            </select>
          </label>
        </div>
        <p class="form-hint">${escapeHtml(worktree.workspace || worktree.path || worktree.summary || '')}</p>
        ${dialog.error ? `<div class="form-error">${escapeHtml(dialog.error)}</div>` : ''}
        <footer class="modal-actions">
          <button class="secondary" type="button" data-action="close-dialog">Cancel</button>
          <button class="primary" type="submit" ${dialog.busy ? 'disabled' : ''}>${dialog.busy ? 'Saving...' : 'Save settings'}</button>
        </footer>
      </form>
    </div>
  `;
}

function renderReviewDenyDialog() {
  const dialog = state.dialog;
  const worktreeItem = findWorktreeItem(dialog.worktreeId);
  const worktree = worktreeItem ? worktreeDetail(worktreeItem) : null;
  const reviewDiff = worktree ? reviewDiffFor(worktree, dialog.path) : { diff: '', source: 'none' };
  const loading = state.fileLoads.has(fileDataKey(dialog.worktreeId, dialog.path));
  return `
    <div class="modal-backdrop" role="dialog" aria-modal="true">
      <form class="modal-panel review-modal" data-form="review-deny">
        <input type="hidden" name="worktree_id" value="${escapeAttr(dialog.worktreeId)}">
        <input type="hidden" name="path" value="${escapeAttr(dialog.path)}">
        <header class="modal-head">
          <h2>Deny review</h2>
          <button class="icon-button" type="button" data-action="close-dialog" aria-label="Close">x</button>
        </header>
        <div class="review-target">
          <code>${escapeHtml(dialog.path)}</code>
          <span>${escapeHtml(loading ? 'loading live diff' : `${reviewDiff.source} diff`)}</span>
        </div>
        <label class="form-field">
          <span>Reason</span>
          <textarea name="reason" rows="5" data-dialog-field="reason" placeholder="Tell the agent what should change">${escapeHtml(dialog.values.reason)}</textarea>
        </label>
        <div class="review-preview">
          <div class="panel-title">Diff sent to agent</div>
          <pre>${escapeHtml(reviewDiffPreview(reviewDiff.diff) || 'No diff available; the reason will still be sent.')}</pre>
        </div>
        ${dialog.error ? `<div class="form-error">${escapeHtml(dialog.error)}</div>` : ''}
        <footer class="modal-actions">
          <button class="secondary" type="button" data-action="close-dialog">Cancel</button>
          <button class="primary danger-primary" type="submit" ${dialog.busy ? 'disabled' : ''}>${dialog.busy ? 'Sending...' : 'Deny and send'}</button>
        </footer>
      </form>
    </div>
  `;
}

function renderChat(messages) {
  const routeScoped = state.route.screen === 'worktree' || state.route.screen === 'plan';
  const canSend = !state.transport.client?.connected || !routeScoped || !!currentRouteChannelId();
  return `
    <div class="chat-list">
      ${messages.map(renderChatItem).join('') || `<div class="empty-state">${canUseFixtures() ? 'No chat messages' : 'No live chat messages'}</div>`}
    </div>
    <form class="composer" data-form="chat">
      <input name="message" autocomplete="off" placeholder="${canSend ? 'Message agent' : 'No agent channel'}" ${canSend ? '' : 'disabled'}>
      <button class="primary small" type="submit" ${canSend ? '' : 'disabled'}>Send</button>
    </form>
  `;
}

function renderChatItem(msg) {
  if (msg.kind === 'thinking') {
    return `
      <div class="chat-activity thinking">
        <div class="chat-meta"><span>Thinking</span><span>${escapeHtml(msg.time)}</span></div>
        <p>${escapeHtml(msg.text)}</p>
      </div>
    `;
  }
  if (msg.kind === 'tool_use') {
    return `
      <div class="chat-activity tool">
        <div class="chat-meta"><span>Tool</span><span>${escapeHtml(msg.time)}</span></div>
        <p><strong>${escapeHtml(msg.name)}</strong> ${escapeHtml(activityPreview(msg.input))}</p>
      </div>
    `;
  }
  if (msg.kind === 'tool_result') {
    return `
      <div class="chat-activity tool-result ${msg.isError ? 'error' : ''}">
        <div class="chat-meta"><span>${msg.isError ? 'Tool error' : 'Tool result'}</span><span>${escapeHtml(msg.time)}</span></div>
        <p>${escapeHtml(msg.text)}</p>
      </div>
    `;
  }
  if (msg.kind === 'activity_end') {
    return `<div class="chat-activity end"><span>${escapeHtml(msg.reason)}</span><span>${escapeHtml(msg.time)}</span></div>`;
  }
  if (msg.kind === 'review_denied') {
    return `
      <div class="chat-review denied">
        <div class="chat-meta"><span>Review denied</span><span>${escapeHtml(msg.time)}</span></div>
        <div class="review-card-head">
          <code>${escapeHtml(msg.file || 'file')}</code>
          ${msg.repoPath ? `<span>${escapeHtml(msg.repoPath)}</span>` : ''}
        </div>
        <p>${escapeHtml(msg.reason || '')}</p>
        ${msg.diff ? `<pre>${escapeHtml(reviewDiffPreview(msg.diff))}</pre>` : ''}
      </div>
    `;
  }
  if (msg.kind === 'interaction_request') {
    const channelId = currentRouteChannelId() || msg.channel_id || '';
    return `
      <div class="chat-interaction">
        <div class="chat-meta"><span>${escapeHtml(interactionTitle(msg.interaction))}</span><span>${escapeHtml(msg.time)}</span></div>
        <p>${escapeHtml(msg.text)}</p>
        ${msg.interaction?.plan ? `<pre class="interaction-plan">${escapeHtml(reviewDiffPreview(msg.interaction.plan))}</pre>` : ''}
        ${renderInteractionControls(msg.interaction, { channelId })}
      </div>
    `;
  }
  return `
    <div class="chat-message ${msg.author === 'You' ? 'mine' : ''}">
      <div class="chat-meta">
        <span>${escapeHtml(msg.author)}</span>
        <span>${escapeHtml(msg.time)}</span>
      </div>
      <p>${escapeHtml(msg.text)}</p>
    </div>
  `;
}

function interactionTitle(interaction) {
  const kind = interaction?.kind || 'question';
  if (kind.includes('approval')) return 'Approval requested';
  if (kind.includes('plan')) return 'Plan review';
  if (kind.includes('review')) return 'Review requested';
  return 'Agent question';
}

function renderTerminal() {
  const lines = canUseFixtures() ? [...WORKTREE.terminal, ...state.terminalLines] : state.terminalLines;
  const canRun = !state.transport.client?.connected || !!firstLiveChannel();
  return `
    <pre class="terminal-output">${lines.length ? lines.map(line => escapeHtml(line)).join('\n') : 'No live terminal output'}</pre>
    <form class="terminal-input" data-form="terminal">
      <input name="command" autocomplete="off" placeholder="${canRun ? 'Command' : 'No agent channel'}" ${canRun ? '' : 'disabled'}>
      <button class="primary small" type="submit" ${canRun ? '' : 'disabled'}>Run</button>
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

function metaPill(label, value) {
  return `
    <div class="meta-pill">
      <span>${escapeHtml(label)}</span>
      <strong>${escapeHtml(String(value || '-'))}</strong>
    </div>
  `;
}

function agentBadge(model) {
  const agent = AGENTS[model] || Object.values(AGENTS).find(item => item.name === model) || {
    label: agentLabel(model),
    name: model || 'device',
    color: colorFor(model || 'device'),
  };
  return `
    <span class="agent-badge" style="--agent:${escapeAttr(agent.color)}" title="${escapeAttr(agent.name)}">
      ${escapeHtml(agent.label)}
    </span>
  `;
}

function agentKey(name) {
  const match = Object.entries(AGENTS).find(([, agent]) => agent.name === name);
  return match?.[0] || 'cs';
}

function agentLabel(model) {
  const value = String(model || 'device').replace(/^claude-|^gpt-/i, '');
  const parts = value.split(/[^a-z0-9]+/i).filter(Boolean);
  if (!parts.length) return 'DV';
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return parts.slice(0, 2).map(part => part[0]).join('').toUpperCase();
}

function colorFor(value) {
  const colors = ['#6366f1', '#0891b2', '#9333ea', '#0f766e', '#e11d48', '#d97706'];
  let hash = 0;
  for (const char of String(value || 'workspace')) {
    hash = ((hash << 5) - hash + char.charCodeAt(0)) | 0;
  }
  return colors[Math.abs(hash) % colors.length];
}

function renderEmpty(label) {
  return `<div class="empty-state">${escapeHtml(label)}</div>`;
}

function renderMissingEntity(title, detail) {
  return `
    <section class="screen narrow-screen">
      <div class="empty-state missing-entity">
        <strong>${escapeHtml(title)}</strong>
        <span>${escapeHtml(detail)}</span>
        <a class="secondary small" href="#/projects" data-route="#/projects">Back to projects</a>
      </div>
    </section>
  `;
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
  if (type === 'hunk' || type === 'meta') return '';
  return '  ';
}

function relativeMessageTime(value) {
  const timestamp = activityTimestamp(value) / 1000;
  if (!Number.isFinite(timestamp) || timestamp <= 0) return 'now';
  const seconds = Math.max(0, Math.floor(Date.now() / 1000 - timestamp));
  if (seconds < 60) return 'now';
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

function activityTimestamp(value) {
  if (typeof value === 'number') return value > 1_000_000_000_000 ? value : value * 1000;
  if (typeof value === 'string' && value) {
    const numeric = Number(value);
    if (Number.isFinite(numeric)) return numeric > 1_000_000_000_000 ? numeric : numeric * 1000;
    const parsed = Date.parse(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return Date.now();
}

function stringifyActivityContent(value) {
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value ?? '', null, 2);
  } catch {
    return String(value ?? '');
  }
}

function activityPreview(value) {
  const text = stringifyActivityContent(value).replace(/\s+/g, ' ').trim();
  if (!text) return '';
  return text.length > 140 ? `${text.slice(0, 137)}...` : text;
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
