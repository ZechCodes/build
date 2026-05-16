import { BuildE2EE } from '../../dashboard/transport/vendor/e2ee.js';

export class V1ProtocolError extends Error {
  constructor(error, frame) {
    super(error?.message || 'v1 request failed');
    this.name = 'V1ProtocolError';
    this.code = error?.code || 'unknown';
    this.details = error?.details || {};
    this.retryable = !!error?.retryable;
    this.frame = frame;
  }
}

export class BuildE2EEV1 extends BuildE2EE {
  constructor() {
    super();
    this._pending = new Map();
  }

  _handleDataFrame(frame) {
    const app = frame?.payload;
    if (!isV1Envelope(app)) {
      super._handleDataFrame(frame);
      return;
    }

    this.dispatchEvent(new CustomEvent('v1:frame', { detail: app }));
    this.dispatchEvent(new CustomEvent(`v1:${app.kind}`, { detail: app }));
    if (app.type) {
      this.dispatchEvent(new CustomEvent(`v1:${app.type}`, { detail: app }));
    }

    if (app.kind === 'response') {
      this._resolveResponse(app);
    } else if (app.kind === 'stream') {
      this._handleStream(app);
    }
  }

  _resolveResponse(app) {
    const pending = this._pending.get(app.ref);
    if (!pending) return;
    clearTimeout(pending.timeout);
    this._pending.delete(app.ref);
    if (app.error) {
      pending.reject(new V1ProtocolError(app.error, app));
      return;
    }
    pending.resolve(app);
  }

  _handleStream(app) {
    const pending = this._pending.get(app.ref);
    if (pending?.onStream) pending.onStream(app);
  }

  disconnect() {
    for (const [id, pending] of this._pending.entries()) {
      clearTimeout(pending.timeout);
      pending.reject(new Error('disconnected'));
      this._pending.delete(id);
    }
    super.disconnect();
  }

  async request(type, options = {}) {
    if (!this.connected) throw new Error('not connected');

    const id = options.id || `req_${crypto.randomUUID().replaceAll('-', '')}`;
    const app = {
      v: 1,
      kind: 'request',
      id,
      type,
      target: {
        device_id: this._deviceId,
        ...(options.target || {}),
      },
      payload: options.payload || {},
      meta: {
        trace_id: id,
        ...(options.meta || {}),
      },
    };

    const promise = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this._pending.delete(id);
        reject(new Error(`${type} timed out`));
      }, options.timeoutMs || 15000);
      this._pending.set(id, {
        resolve,
        reject,
        timeout,
        onStream: options.onStream,
      });
    });

    try {
      await this.send(app);
    } catch (err) {
      const pending = this._pending.get(id);
      if (pending) {
        clearTimeout(pending.timeout);
        this._pending.delete(id);
      }
      throw err;
    }

    return promise;
  }

  async closeV1() {
    if (!this.connected) return;
    const id = `cls_${crypto.randomUUID().replaceAll('-', '')}`;
    await this.send({
      v: 1,
      kind: 'close',
      id,
      target: { device_id: this._deviceId },
      payload: {},
      meta: {},
    });
    this.disconnect();
  }

  hello() {
    return this.request('protocol.hello');
  }

  listChannels() {
    return this.request('channel.list');
  }

  dashboardSnapshot() {
    return this.request('dashboard.snapshot');
  }

  listProjects() {
    return this.request('project.list');
  }

  listWorktrees(projectId = null) {
    return this.request('worktree.list', {
      payload: projectId ? { project_id: projectId } : {},
    });
  }

  createWorktree(projectId, options = {}) {
    const { timeoutMs, ...payload } = options;
    return this.request('worktree.create', {
      payload: {
        ...payload,
        project_id: projectId,
      },
      timeoutMs: timeoutMs || 30000,
    });
  }

  worktreeSnapshot(worktreeId) {
    return this.request('worktree.snapshot', {
      payload: { worktree_id: worktreeId },
    });
  }

  listPlans(options = {}) {
    return this.request('plan.list', {
      payload: {
        ...(options.projectId ? { project_id: options.projectId } : {}),
        ...(options.worktreeId ? { worktree_id: options.worktreeId } : {}),
      },
    });
  }

  createChannel(name, agent = {}) {
    return this.request('channel.create', {
      payload: { name, agent },
    });
  }

  updateChannel(channelId, payload) {
    return this.request('channel.update', {
      target: { channel_id: channelId },
      payload,
    });
  }

  deleteChannel(channelId) {
    return this.request('channel.delete', {
      target: { channel_id: channelId },
    });
  }

  listMessages(channelId, options = {}) {
    return this.request('message.list', {
      target: { channel_id: channelId },
      payload: {
        limit: options.limit || 50,
        before: options.before || null,
      },
    });
  }

  sendMessage(channelId, content, options = {}) {
    return this.request('message.send', {
      target: { channel_id: channelId },
      payload: {
        content,
        attachments: options.attachments || [],
        agent_options: options.agent_options || {},
      },
    });
  }

  markRead(channelId, messageIds) {
    return this.request('message.mark_read', {
      target: { channel_id: channelId },
      payload: { message_ids: messageIds },
    });
  }

  markSeen(channelId) {
    return this.request('message.mark_seen', {
      target: { channel_id: channelId },
    });
  }

  listActivity(channelId) {
    return this.request('activity.list', {
      target: { channel_id: channelId },
    });
  }

  listHarnesses() {
    return this.request('agent.harnesses');
  }

  listWorkers() {
    return this.request('agent.workers');
  }

  startAgent(channelId, payload) {
    return this.request('agent.start', {
      target: { channel_id: channelId },
      payload,
    });
  }

  stopAgent(channelId) {
    return this.request('agent.stop', {
      target: { channel_id: channelId },
    });
  }

  cancelTurn(channelId) {
    return this.request('agent.cancel_turn', {
      target: { channel_id: channelId },
    });
  }

  restartAgent(channelId) {
    return this.request('agent.restart', {
      target: { channel_id: channelId },
    });
  }

  resetAgentContext(channelId) {
    return this.request('session.reset_agent_context', {
      target: { channel_id: channelId },
    });
  }

  compactAgentContext(channelId) {
    return this.request('session.compact_agent_context', {
      target: { channel_id: channelId },
    });
  }

  respondToInteraction(channelId, interactionId, response) {
    return this.request('interaction.respond', {
      target: { channel_id: channelId },
      payload: { interaction_id: interactionId, response },
    });
  }

  listComplications(channelId) {
    return this.request('complication.list', {
      target: { channel_id: channelId },
    });
  }

  invokeComplication(channelId, complicationId, optionId) {
    return this.request('complication.invoke', {
      target: { channel_id: channelId },
      payload: { complication_id: complicationId, option_id: optionId },
    });
  }

  terminalExec(channelId, command, options = {}) {
    return this.request('terminal.exec', {
      target: { channel_id: channelId },
      payload: {
        command,
        cwd: options.cwd || '',
      },
      onStream: options.onStream,
      timeoutMs: options.timeoutMs || 60000,
    });
  }

  terminalKill(channelId, requestId) {
    return this.request('terminal.kill', {
      target: { channel_id: channelId },
      payload: { request_id: requestId },
    });
  }

  terminalComplete(channelId, line, options = {}) {
    return this.request('terminal.complete', {
      target: { channel_id: channelId },
      payload: {
        line,
        cursor: options.cursor ?? line.length,
        cwd: options.cwd || '',
      },
    });
  }

  fileTree(channelId, path = '') {
    return this.request('file.tree', {
      target: { channel_id: channelId },
      payload: { path },
    });
  }

  fileChanges(channelId, options = {}) {
    return this.request('file.changes', {
      target: { channel_id: channelId },
      payload: {
        repo_path: options.repoPath || '',
        base_ref: options.baseRef,
        head_ref: options.headRef,
      },
    });
  }

  fileCommits(channelId, options = {}) {
    return this.request('file.commits', {
      target: { channel_id: channelId },
      payload: {
        repo_path: options.repoPath || '',
        limit: options.limit || 25,
      },
    });
  }

  fileRead(channelId, path, options = {}) {
    return this.request('file.read', {
      target: { channel_id: channelId },
      payload: {
        path,
        offset: options.offset || 0,
        limit: options.limit,
      },
      onStream: options.onStream,
    });
  }

  fileDiff(channelId, path, options = {}) {
    return this.request('file.diff', {
      target: { channel_id: channelId },
      payload: {
        path,
        repo_path: options.repoPath,
        staged: !!options.staged,
        base_ref: options.baseRef,
        head_ref: options.headRef,
      },
    });
  }

  urlFetch(payload) {
    return this.request('url.fetch', { payload });
  }

  async uploadFileV1(channelId, file, destination = { kind: 'scratch' }) {
    if (!this.connected) throw new Error('not connected');
    const uploadId = `upl_${crypto.randomUUID().replaceAll('-', '')}`;
    const fileBuffer = await file.arrayBuffer();
    const hashBuffer = await crypto.subtle.digest('SHA-256', fileBuffer);
    const sha256 = [...new Uint8Array(hashBuffer)]
      .map(byte => byte.toString(16).padStart(2, '0'))
      .join('');

    const create = await this.request('upload.create', {
      target: { channel_id: channelId },
      payload: {
        upload_id: uploadId,
        filename: file.name,
        mime_type: file.type || 'application/octet-stream',
        size: file.size,
        sha256,
        destination,
      },
      timeoutMs: 30000,
    });

    const chunkSize = create.payload?.chunk_size || 64 * 1024;
    const bytes = new Uint8Array(fileBuffer);
    const totalChunks = Math.max(1, Math.ceil(bytes.length / chunkSize));
    for (let index = 0; index < totalChunks; index++) {
      const chunk = bytes.slice(index * chunkSize, Math.min(bytes.length, (index + 1) * chunkSize));
      await this.request('upload.write_chunk', {
        target: { channel_id: channelId },
        payload: {
          upload_id: uploadId,
          index,
          data: this._toB64(chunk),
        },
        timeoutMs: 30000,
      });
      this.dispatchEvent(new CustomEvent('v1:upload_progress', {
        detail: { upload_id: uploadId, index: index + 1, total: totalChunks },
      }));
    }

    return this.request('upload.complete', {
      target: { channel_id: channelId },
      payload: { upload_id: uploadId },
      timeoutMs: 30000,
    });
  }
}

function isV1Envelope(value) {
  return value && typeof value === 'object' && value.v === 1 && typeof value.kind === 'string';
}
