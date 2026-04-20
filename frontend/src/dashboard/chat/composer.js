import { state } from '../state.js';
import { getActiveE2EE } from '../e2ee/bridge.js';
import { clearChannelDraft, saveChannelState } from '../channels/state-store.js';
import { clearPendingFiles } from './uploads.js';
import { handleSentMessageScroll } from '../console/view.js';

// Functions still in legacy.js (until Wave B) — bridged via window:
//   window.appendMessage, window.resolvePendingPlanReviews, window.updateStopButton

const _isMobile = ('ontouchstart' in window || navigator.maxTouchPoints > 0);

export async function sendChatMessage() {
  const input = document.getElementById('chat-input');
  const content = input.value.trim();
  const hasFiles = state.pendingFiles.length > 0;
  const _sendConn = getActiveE2EE();
  if ((!content && !hasFiles) || !state.chatCurrentChannel || !_sendConn || !_sendConn.connected) return;

  // Capture channel at send time so async uploads don't target the wrong channel.
  const channelId = state.chatCurrentChannel;

  input.value = '';
  input.style.height = 'auto';
  clearChannelDraft(channelId);

  // Upload any staged files first.
  let attachments = null;
  if (hasFiles) {
    const files = [...state.pendingFiles];
    clearPendingFiles();
    attachments = [];
    for (const file of files) {
      try {
        const result = await _sendConn.uploadFile(channelId, file);
        attachments.push({
          file_id: result.file_id,
          filename: result.filename,
          size: result.size,
          mime_type: result.mime_type,
        });
      } catch (err) {
        console.error('[Chat] File upload failed:', err);
      }
    }
    if (!attachments.length) attachments = null;
  }

  const messageContent = content || (attachments ? `Sent ${attachments.length} file(s)` : '');
  if (!messageContent) return;

  // Dismiss all pending suggested action buttons.
  document.querySelectorAll('.msg-suggestions .suggestion-btn:not(.selected):not(.dismissed)').forEach(b => b.classList.add('dismissed'));

  // Optimistically render the message.
  const tempMsg = {
    id: crypto.randomUUID(),
    channel_id: channelId,
    sender: 'client',
    content: messageContent,
    created_at: Date.now() / 1000,
    attachments,
  };
  const msgs = state.chatMessages.get(channelId) || [];
  msgs.push(tempMsg);
  state.chatMessages.set(channelId, msgs);
  window.appendMessage?.(tempMsg);
  const _sentEl = document.getElementById('chat-messages').lastElementChild;
  if (_sentEl) handleSentMessageScroll(_sentEl);

  // Auto-resolve any pending plan review cards (agent cancels interactions on new messages).
  window.resolvePendingPlanReviews?.();

  try {
    const payload = { action: 'message', channel_id: channelId, content: messageContent };
    if (attachments) payload.attachments = attachments;
    if (state.channelPlanMode.get(channelId)) payload.plan_mode = true;
    const _chData = state.chatChannels.get(channelId);
    if (_chData?.model) payload.model = _chData.model;
    if (_chData?.effort) payload.effort = _chData.effort;
    const realMessageId = await _sendConn.send(payload);
    const el = document.querySelector(`[data-msg-id="${tempMsg.id}"]`);
    if (el) el.dataset.msgId = realMessageId;
    tempMsg.id = realMessageId;
  } catch (err) {
    console.error('[Chat] Send failed:', err);
  }
}

document.getElementById('chat-send-btn')?.addEventListener('click', sendChatMessage);

document.getElementById('chat-stop-btn')?.addEventListener('click', () => {
  const _stopConn = getActiveE2EE();
  if (!state.chatCurrentChannel || !_stopConn?.connected) return;
  // Device handles two-phase stop: graceful cancel → 3s → process kill.
  _stopConn.stopAgent(state.chatCurrentChannel).catch(() => {});
  state.channelAgentActive.set(state.chatCurrentChannel, false);
  window.updateStopButton?.();
});

document.getElementById('chat-input')?.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey && !_isMobile) {
    e.preventDefault();
    sendChatMessage();
  }
});

// Save draft input to localStorage on every change.
document.getElementById('chat-input')?.addEventListener('input', () => {
  if (!state.chatCurrentChannel) return;
  saveChannelState(state.chatCurrentChannel, { draft: document.getElementById('chat-input').value });
});
