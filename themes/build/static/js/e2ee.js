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

  async createChannel(name) {
    return this.send({ action: 'create_channel', name });
  }

  async getMessages(channelId, limit = 50, before = null) {
    return this.send({ action: 'get_messages', channel_id: channelId, limit, before });
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
