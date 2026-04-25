export const READ_INTERACTION_DELAY_MS = 250;

export function isReadableMessage(msg) {
  return !!msg?.id && msg.sender !== 'client' && !msg.read_at;
}

export function canMarkMessageRead({
  msg,
  appearedAt,
  lastInteractionAt,
  messageBottom,
  viewportBottom,
  delayMs = READ_INTERACTION_DELAY_MS,
}) {
  if (!isReadableMessage(msg)) return false;
  if (!Number.isFinite(appearedAt) || !Number.isFinite(lastInteractionAt)) return false;
  if (lastInteractionAt < appearedAt + delayMs) return false;
  return messageBottom <= viewportBottom + 1;
}

export function parseMessageMetadata(raw) {
  if (!raw) return null;
  if (typeof raw === 'object') return raw;
  try { return JSON.parse(raw); } catch (_) { return null; }
}

export function isUnresolvedInteractionMessage(msg) {
  const meta = parseMessageMetadata(msg?.metadata);
  return !!meta?.interaction_id && !meta?.resolved_at;
}

export function latestNonClientMessage(msgs) {
  for (let i = (msgs?.length || 0) - 1; i >= 0; i--) {
    const msg = msgs[i];
    if (msg && msg.sender !== 'client') return msg;
  }
  return null;
}

export function deriveUnreadFromMessages(msgs) {
  let count = 0;
  for (const msg of msgs || []) {
    if (isReadableMessage(msg)) count += 1;
  }
  const latest = latestNonClientMessage(msgs);
  return {
    count,
    hasInteraction: isUnresolvedInteractionMessage(latest),
    latest,
  };
}
