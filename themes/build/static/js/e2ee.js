/**
 * Browser-side E2EE client for Build chat.
 *
 * Implements the secure transport protocol using libsodium-wrappers
 * (expected to be loaded globally as `window.sodium`).
 *
 * Usage:
 *   const client = new BuildE2EE();
 *   await client.ready();
 *   await client.connect(deviceId);
 *   await client.send(channelId, "Hello!");
 *   client.on('message', (msg) => { ... });
 */

const PROTOCOL_VERSION = 1;
const NONCE_BYTES = 24;
const SESSION_KEY_BYTES = 32;

class BuildE2EE extends EventTarget {
  constructor() {
    super();
    this._sodium = null;
    this._sessionId = null;
    this._sessionKeyB64 = null;
    this._sessionKey = null;
    this._deviceId = null;
    this._connected = false;
    this._notificationHandler = null;
  }

  async ready() {
    if (typeof sodium === 'undefined') {
      throw new Error('libsodium-wrappers must be loaded before BuildE2EE');
    }
    await sodium.ready;
    this._sodium = sodium;
  }

  // ---- Base64 helpers (using libsodium's no-padding variant) ----
  _toB64(bytes) {
    return this._sodium.to_base64(bytes, this._sodium.base64_variants.ORIGINAL_NO_PADDING);
  }

  _fromB64(str) {
    return this._sodium.from_base64(str, this._sodium.base64_variants.ORIGINAL_NO_PADDING);
  }

  _canonicalJsonBytes(obj) {
    return this._sodium.from_string(JSON.stringify(this._sortKeys(obj)));
  }

  _sortKeys(value) {
    if (Array.isArray(value)) return value.map(v => this._sortKeys(v));
    if (value && typeof value === 'object') {
      return Object.keys(value).sort().reduce((acc, k) => {
        acc[k] = this._sortKeys(value[k]);
        return acc;
      }, {});
    }
    return value;
  }

  _parseJsonBytes(bytes) {
    return JSON.parse(this._sodium.to_string(bytes));
  }

  // ---- Session Bootstrap ----

  async connect(deviceId) {
    this._deviceId = deviceId;

    // 1. Fetch device transport key from relay.
    const keyResp = await fetch(`/api/devices/${deviceId}/transport-key`);
    if (!keyResp.ok) {
      const err = await keyResp.json().catch(() => ({ error: 'failed to fetch transport key' }));
      throw new Error(err.error || 'failed to fetch transport key');
    }
    const keyData = await keyResp.json();
    const transportPubKey = this._fromB64(keyData.transport_public_key);

    // 2. Generate session key.
    this._sessionId = crypto.randomUUID();
    this._sessionKey = this._sodium.randombytes_buf(SESSION_KEY_BYTES);
    this._sessionKeyB64 = this._toB64(this._sessionKey);

    // 3. Wrap session key in sealed box to device transport key.
    const wrappedSessionKey = this._sodium.crypto_box_seal(this._sessionKey, transportPubKey);

    const sessionInit = {
      session_id: this._sessionId,
      device_id: deviceId,
      wrapped_session_key: this._toB64(wrappedSessionKey),
    };

    // 4. Listen for SSE notifications before sending init.
    this._startListening();

    // 5. Send session_init to relay.
    const initResp = await fetch('/api/devices/e2ee/session-init', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        device_id: deviceId,
        session_id: this._sessionId,
        session_init: sessionInit,
      }),
    });

    if (!initResp.ok) {
      const err = await initResp.json().catch(() => ({ error: 'session_init failed' }));
      throw new Error(err.error || 'session_init failed');
    }

    // 6. Wait for session_accept (with timeout).
    await this._waitForSessionAccept(5000);

    this._connected = true;
    this.dispatchEvent(new CustomEvent('connected', { detail: { sessionId: this._sessionId } }));
  }

  _startListening() {
    // Listen for Skrift SSE notifications containing E2EE envelopes.
    this._notificationHandler = (event) => {
      const notification = event.detail;
      if (notification.type !== 'build:e2ee:envelope') return;
      if (notification.session_id !== this._sessionId) return;

      // Prevent Skrift from showing this as a UI notification.
      event.preventDefault();

      const envelope = notification.envelope;
      this._handleEnvelope(envelope);
    };
    document.addEventListener('sk:notification', this._notificationHandler);
  }

  _waitForSessionAccept(timeoutMs) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        reject(new Error('session_accept timed out'));
      }, timeoutMs);

      const handler = (event) => {
        if (event.type !== 'session_accepted') return;
        clearTimeout(timeout);
        this.removeEventListener('session_accepted', handler);
        resolve();
      };
      this.addEventListener('session_accepted', handler);
    });
  }

  _handleEnvelope(envelope) {
    if (!this._sessionKey) return;

    try {
      // Validate envelope shape.
      const nonce = this._fromB64(envelope.nonce);
      const ciphertext = this._fromB64(envelope.ciphertext);

      if (nonce.length !== NONCE_BYTES) throw new Error('bad nonce length');

      // Decrypt.
      const plaintext = this._sodium.crypto_secretbox_open_easy(ciphertext, nonce, this._sessionKey);
      const frame = this._parseJsonBytes(plaintext);

      // Validate session_id match.
      if (frame.session_id !== this._sessionId) {
        throw new Error('session_id mismatch');
      }

      // Check if this is a session_accept (no frame_type, just session_id).
      if (!frame.frame_type) {
        // This is a session_accept.
        this.dispatchEvent(new Event('session_accepted'));
        return;
      }

      if (frame.frame_type === 'close') {
        this.disconnect();
        return;
      }

      if (frame.frame_type === 'data') {
        this._handleDataFrame(frame);
      }
    } catch (err) {
      console.error('[E2EE] Failed to decrypt envelope:', err);
      // On crypto failure, close session.
      this.disconnect();
    }
  }

  _handleDataFrame(frame) {
    const payload = frame.payload || {};
    const action = payload.action;

    if (action === 'channel_list') {
      this.dispatchEvent(new CustomEvent('channel_list', { detail: payload.channels }));
    } else if (action === 'channel_created') {
      this.dispatchEvent(new CustomEvent('channel_created', { detail: payload.channel }));
    } else if (action === 'messages') {
      this.dispatchEvent(new CustomEvent('messages', {
        detail: { channel_id: payload.channel_id, messages: payload.messages },
      }));
    } else if (action === 'message') {
      this.dispatchEvent(new CustomEvent('message', { detail: payload.message }));
    } else if (action === 'delivered') {
      this.dispatchEvent(new CustomEvent('delivered', {
        detail: { message_id: payload.message_id, channel_id: payload.channel_id },
      }));
    } else if (action === 'read') {
      this.dispatchEvent(new CustomEvent('read', {
        detail: { message_ids: payload.message_ids, channel_id: payload.channel_id },
      }));
    } else if (action === 'harness_list') {
      this.dispatchEvent(new CustomEvent('harness_list', { detail: payload.harnesses }));
    } else if (action === 'agent_started') {
      this.dispatchEvent(new CustomEvent('agent_started', { detail: payload }));
    } else if (action === 'agent_stopped') {
      this.dispatchEvent(new CustomEvent('agent_stopped', { detail: payload }));
    } else if (action === 'agent_restarted') {
      this.dispatchEvent(new CustomEvent('agent_restarted', { detail: payload }));
    } else if (action === 'channel_renamed') {
      this.dispatchEvent(new CustomEvent('channel_renamed', { detail: payload }));
    } else if (action === 'channel_deleted') {
      this.dispatchEvent(new CustomEvent('channel_deleted', { detail: payload }));
    } else if (action === 'worker_list') {
      this.dispatchEvent(new CustomEvent('worker_list', { detail: payload.workers }));
    } else if (action === 'activity_history') {
      this.dispatchEvent(new CustomEvent('activity_history', {
        detail: { channel_id: payload.channel_id, entries: payload.entries, total_tool_uses: payload.total_tool_uses },
      }));
    } else if (action === 'agent_event') {
      this.dispatchEvent(new CustomEvent('agent_event', { detail: payload }));
    } else if (action === 'chunk_ack') {
      this.dispatchEvent(new CustomEvent('chunk_ack', {
        detail: { file_id: payload.file_id, chunk_index: payload.chunk_index },
      }));
    } else if (action === 'upload_accepted') {
      this.dispatchEvent(new CustomEvent('upload_accepted', {
        detail: { file_id: payload.file_id, filename: payload.filename, size: payload.size, path: payload.path },
      }));
    } else if (action === 'upload_error') {
      this.dispatchEvent(new CustomEvent('upload_error', {
        detail: { file_id: payload.file_id, error: payload.error },
      }));
    } else if (action === 'complication:update') {
      this.dispatchEvent(new CustomEvent('complication_update', { detail: payload }));
    } else if (action === 'complication:remove') {
      this.dispatchEvent(new CustomEvent('complication_remove', { detail: payload }));
    } else if (action === 'complications') {
      this.dispatchEvent(new CustomEvent('complications', { detail: payload }));
    } else if (action === 'system_message') {
      this.dispatchEvent(new CustomEvent('system_message', {
        detail: { channel_id: payload.channel_id, text: payload.text },
      }));
    } else if (action === 'plan_mode_updated') {
      this.dispatchEvent(new CustomEvent('plan_mode_updated', {
        detail: { channel_id: payload.channel_id, plan_mode: payload.plan_mode },
      }));
    } else if (action === 'session_reset') {
      this.dispatchEvent(new CustomEvent('session_reset', {
        detail: { channel_id: payload.channel_id },
      }));
    } else if (action === 'compact_started') {
      this.dispatchEvent(new CustomEvent('compact_started', {
        detail: { channel_id: payload.channel_id },
      }));
    } else if (action === 'error') {
      this.dispatchEvent(new CustomEvent('e2ee_error', { detail: payload.error }));
    }
  }

  // ---- Sending ----

  async send(payload) {
    if (!this._connected) throw new Error('not connected');

    const envelope = this._encryptFrame({
      frame_type: 'data',
      sender: 'client',
      payload,
    });

    const resp = await fetch('/api/devices/e2ee/send', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        session_id: this._sessionId,
        envelope,
      }),
    });

    if (!resp.ok) {
      const err = await resp.json().catch(() => ({ error: 'send failed' }));
      throw new Error(err.error || 'send failed');
    }

    return envelope._messageId; // Attached by _encryptFrame for tracking.
  }

  _encryptFrame(frameFields) {
    const messageId = crypto.randomUUID();
    const innerFrame = {
      session_id: this._sessionId,
      message_id: messageId,
      frame_type: frameFields.frame_type,
      sender: frameFields.sender,
      created_at: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
      payload: frameFields.payload,
    };

    const nonce = this._sodium.randombytes_buf(NONCE_BYTES);
    const plaintext = this._canonicalJsonBytes(innerFrame);
    const ciphertext = this._sodium.crypto_secretbox_easy(plaintext, nonce, this._sessionKey);

    const envelope = {
      version: PROTOCOL_VERSION,
      session_id: this._sessionId,
      route_to: 'device',
      nonce: this._toB64(nonce),
      ciphertext: this._toB64(ciphertext),
    };
    envelope._messageId = messageId;
    return envelope;
  }

  // ---- High-level API ----

  async listChannels() {
    return this.send({ action: 'list_channels' });
  }

  async createChannel(name, opts = {}) {
    const payload = { action: 'create_channel', name };
    if (opts.harness) payload.harness = opts.harness;
    if (opts.model) payload.model = opts.model;
    if (opts.system_prompt) payload.system_prompt = opts.system_prompt;
    if (opts.working_directory) payload.working_directory = opts.working_directory;
    return this.send(payload);
  }

  async getMessages(channelId, limit = 50, before = null) {
    return this.send({ action: 'get_messages', channel_id: channelId, limit, before });
  }

  async getActivity(channelId) {
    return this.send({ action: 'get_activity', channel_id: channelId });
  }

  async getComplications(channelId) {
    return this.send({ action: 'get_complications', channel_id: channelId });
  }

  async sendMessage(channelId, content) {
    const messageId = crypto.randomUUID();
    const envelope = this._encryptFrame({
      frame_type: 'data',
      sender: 'client',
      payload: { action: 'message', channel_id: channelId, content },
    });
    // Override message_id to track it.
    // Actually the message_id is in the encrypted inner frame, so we use the one from _encryptFrame.
    await fetch('/api/devices/e2ee/send', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        session_id: this._sessionId,
        envelope,
      }),
    });
    return envelope._messageId;
  }

  async markRead(messageIds) {
    return this.send({ action: 'mark_read', message_ids: messageIds });
  }

  async listHarnesses() {
    return this.send({ action: 'list_harnesses' });
  }

  async startAgent(channelId, harness, model, opts = {}) {
    const payload = { action: 'start_agent', channel_id: channelId, harness, model };
    if (opts.system_prompt) payload.system_prompt = opts.system_prompt;
    if (opts.working_directory) payload.working_directory = opts.working_directory;
    return this.send(payload);
  }

  async stopAgent(channelId) {
    return this.send({ action: 'stop_agent', channel_id: channelId });
  }

  async restartAgent(channelId) {
    return this.send({ action: 'restart_agent', channel_id: channelId });
  }

  async renameChannel(channelId, name) {
    return this.send({ action: 'rename_channel', channel_id: channelId, name });
  }

  async deleteChannel(channelId) {
    return this.send({ action: 'delete_channel', channel_id: channelId });
  }

  async resetSession(channelId) {
    return this.send({ action: 'reset_session', channel_id: channelId });
  }

  async compactSession(channelId) {
    return this.send({ action: 'compact_session', channel_id: channelId });
  }

  async markSeen(channelId) {
    return this.send({ action: 'mark_seen', channel_id: channelId });
  }

  async listWorkers() {
    return this.send({ action: 'list_workers' });
  }

  // ---- File Upload ----

  /**
   * Upload a file to the device via E2EE chunked transfer.
   *
   * @param {string} channelId - Channel to associate the upload with.
   * @param {File} file - The File object to upload.
   * @returns {Promise<{file_id: string, filename: string, size: number, mime_type: string}>}
   */
  async uploadFile(channelId, file) {
    if (!this._connected) throw new Error('not connected');

    const CHUNK_SIZE = 180 * 1024; // 180 KB raw per chunk
    const fileId = crypto.randomUUID();
    const totalChunks = Math.ceil(file.size / CHUNK_SIZE) || 1;
    const arrayBuffer = await file.arrayBuffer();
    const fileBytes = new Uint8Array(arrayBuffer);

    // Compute SHA-256 of the original file.
    const hashBuffer = await crypto.subtle.digest('SHA-256', fileBytes);
    const sha256 = Array.from(new Uint8Array(hashBuffer))
      .map(b => b.toString(16).padStart(2, '0'))
      .join('');

    this.dispatchEvent(new CustomEvent('upload_progress', {
      detail: { file_id: fileId, filename: file.name, progress: 0, total_chunks: totalChunks },
    }));

    // Send chunks sequentially, waiting for ack after each.
    for (let i = 0; i < totalChunks; i++) {
      const start = i * CHUNK_SIZE;
      const end = Math.min(start + CHUNK_SIZE, file.size);
      const chunkData = fileBytes.slice(start, end);
      const chunkB64 = this._toB64(chunkData);

      await this.send({
        action: 'upload_chunk',
        file_id: fileId,
        channel_id: channelId,
        filename: file.name,
        mime_type: file.type || 'application/octet-stream',
        total_size: file.size,
        total_chunks: totalChunks,
        chunk_index: i,
        data: chunkB64,
      });

      // Wait for chunk_ack from device.
      await this._waitForChunkAck(fileId, i, 30000);

      this.dispatchEvent(new CustomEvent('upload_progress', {
        detail: {
          file_id: fileId,
          filename: file.name,
          progress: (i + 1) / totalChunks,
          total_chunks: totalChunks,
          chunks_done: i + 1,
        },
      }));
    }

    // Send upload_complete.
    await this.send({
      action: 'upload_complete',
      file_id: fileId,
      channel_id: channelId,
      sha256,
    });

    // Wait for upload_accepted.
    const result = await this._waitForUploadAccepted(fileId, 15000);

    this.dispatchEvent(new CustomEvent('upload_done', {
      detail: { file_id: fileId, filename: file.name, size: file.size },
    }));

    return {
      file_id: fileId,
      filename: file.name,
      size: file.size,
      mime_type: file.type || 'application/octet-stream',
      path: result.path,
    };
  }

  _waitForChunkAck(fileId, chunkIndex, timeoutMs) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.removeEventListener('chunk_ack', handler);
        reject(new Error(`chunk_ack timed out for chunk ${chunkIndex}`));
      }, timeoutMs);

      const handler = (event) => {
        const d = event.detail;
        if (d.file_id === fileId && d.chunk_index === chunkIndex) {
          clearTimeout(timeout);
          this.removeEventListener('chunk_ack', handler);
          resolve();
        }
      };
      this.addEventListener('chunk_ack', handler);
    });
  }

  _waitForUploadAccepted(fileId, timeoutMs) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.removeEventListener('upload_accepted', handler);
        this.removeEventListener('upload_error', errHandler);
        reject(new Error('upload_accepted timed out'));
      }, timeoutMs);

      const handler = (event) => {
        const d = event.detail;
        if (d.file_id === fileId) {
          clearTimeout(timeout);
          this.removeEventListener('upload_accepted', handler);
          this.removeEventListener('upload_error', errHandler);
          resolve(d);
        }
      };

      const errHandler = (event) => {
        const d = event.detail;
        if (d.file_id === fileId) {
          clearTimeout(timeout);
          this.removeEventListener('upload_accepted', handler);
          this.removeEventListener('upload_error', errHandler);
          reject(new Error(d.error || 'upload rejected'));
        }
      };

      this.addEventListener('upload_accepted', handler);
      this.addEventListener('upload_error', errHandler);
    });
  }

  async sendInteractionResponse(channelId, interactionId, selectedOption, freeformResponse) {
    return this.send({
      action: 'interaction_response',
      channel_id: channelId,
      interaction_id: interactionId,
      selected_option: selectedOption || null,
      freeform_response: freeformResponse || null,
    });
  }

  disconnect() {
    this._connected = false;
    this._sessionKey = null;
    this._sessionKeyB64 = null;
    this._sessionId = null;
    if (this._notificationHandler) {
      document.removeEventListener('sk:notification', this._notificationHandler);
      this._notificationHandler = null;
    }
    this.dispatchEvent(new Event('disconnected'));
  }

  get connected() { return this._connected; }
  get sessionId() { return this._sessionId; }
}

// Export globally.
window.BuildE2EE = BuildE2EE;
