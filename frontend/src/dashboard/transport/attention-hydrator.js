// On connect / reconnect, populate the Attention sidebar section
// from whatever message history the bridge hands us. The live bus
// events (message.received, interaction.requested) only see things
// happening _now_; they don't backfill state that already existed
// when the page loaded.
//
// Hook points:
//   channel.list  → fires on every E2EE connect (and reconnect).
//                   For each channel in the list, request the
//                   recent message history.
//   message.bulk → fires when the bridge returns a get_messages
//                   response. Derive (unread count, hasInteraction)
//                   from the returned messages and hydrate the
//                   unreadStore accordingly.

import { bus } from '../core/bus.js';
import { unreadStore } from '../domain/unread-store.js';
import { presenceStore, SESSION_GAP_MS } from '../domain/presence-store.js';
import { channelsStore } from '../domain/channels-store.js';
import { log } from '../core/log.js';

const plog = log('attention-hydrator');
const HISTORY_LIMIT = 200;

let bound = false;

export function bindAttentionHydrator() {
  if (bound) return;
  bound = true;

  bus.on('channel.list', ({ channels }) => {
    if (!Array.isArray(channels)) return;
    for (const ch of channels) {
      if (!ch?.id) continue;
      bus.emit('intent.get_messages', {
        channelId: ch.id,
        limit: HISTORY_LIMIT,
        before: null,
      });
    }
  });

  bus.on('message.bulk', ({ channelId, msgs }) => {
    if (!channelId) return;
    const channel = channelsStore.get(channelId);
    const { count, hasInteraction, latestActivityMs, sessionStartMs } = derive(msgs, channel);
    plog.debug('hydrate', channelId, { count, hasInteraction, latestActivityMs, sessionStartMs });
    unreadStore.hydrate(channelId, count, hasInteraction);
    if (latestActivityMs > 0) {
      presenceStore.hydrateLastActive(channelId, latestActivityMs);
    }
    if (sessionStartMs > 0) {
      presenceStore.hydrateSessionStart(channelId, sessionStartMs);
    }
  });
}

/**
 * Derive per-channel attention signals from a bulk message list.
 * Returns `{ count, hasInteraction, latestActivityMs, sessionStartMs }`:
 *   - `count`: unread messages — any non-client message that
 *      postdates the channel's `last_seen_at` AND has no `read_at`.
 *      If no channel / last_seen_at is available, falls back to
 *      the `!read_at` test alone (old behavior).
 *   - `hasInteraction`: only true when the LATEST non-client
 *      message is an unresolved interaction (the agent is
 *      currently waiting on the user).
 *   - `latestActivityMs`: millisecond timestamp of the most recent
 *      message of any sender (user or agent). Feeds the Recent
 *      sidebar's time-windowed sort so a user sending a message
 *      bumps the channel just like an agent message does.
 *   - `sessionStartMs`: timestamp of the first message after the
 *      most recent ≥`SESSION_GAP_MS` activity gap (or the loaded
 *      history's earliest message, when no such gap exists in the
 *      window). Anchors the Recent sidebar's primary sort so a
 *      channel's position holds steady across a working session.
 */
export function derive(msgs, channel) {
  const lastSeenMs = timeOf(channel?.last_seen_at);
  let count = 0;
  let latestActivityMs = 0;
  for (const m of msgs || []) {
    const createdMs = timeOf(m?.created_at);
    if (createdMs > latestActivityMs) latestActivityMs = createdMs;
    if (m?.sender === 'client') continue;
    // If we know the user's last_seen_at, that's authoritative —
    // messages after it are unread, messages before it are seen.
    // Otherwise fall back to the per-message `read_at` flag.
    const unread = lastSeenMs > 0
      ? createdMs > lastSeenMs
      : !m.read_at;
    if (unread) count += 1;
  }

  let hasInteraction = false;
  for (let i = (msgs?.length || 0) - 1; i >= 0; i--) {
    const m = msgs[i];
    if (!m || m.sender === 'client') continue;  // client messages don't supersede
    const meta = parseMetadata(m.metadata);
    if (meta?.interaction_id && !meta?.resolved_at) {
      hasInteraction = true;
    }
    // First non-client message from the tail — whatever its kind
    // — decides. Anything below is "older" and irrelevant.
    break;
  }

  // Walk messages chronologically (the wire order is ascending; we
  // sort defensively in case a future caller passes them otherwise)
  // and find the most recent SESSION_GAP_MS-or-longer quiet stretch.
  // The first message AFTER that gap is the session anchor; with no
  // gap in the loaded history, the earliest message is.
  const stamps = [];
  for (const m of msgs || []) {
    const t = timeOf(m?.created_at);
    if (t > 0) stamps.push(t);
  }
  stamps.sort((a, b) => a - b);
  let sessionStartMs = 0;
  for (let i = 0; i < stamps.length; i++) {
    if (i === 0 || stamps[i] - stamps[i - 1] >= SESSION_GAP_MS) {
      sessionStartMs = stamps[i];
    }
  }

  return { count, hasInteraction, latestActivityMs, sessionStartMs };
}

function parseMetadata(raw) {
  if (!raw) return null;
  if (typeof raw === 'object') return raw;
  try { return JSON.parse(raw); } catch (_) { return null; }
}

/**
 * Convert a timestamp in any of the shapes the wire uses
 * (ISO string, Date, numeric seconds, numeric milliseconds) into
 * milliseconds since epoch. Returns 0 for falsy / unparseable.
 */
function timeOf(raw) {
  if (!raw) return 0;
  if (raw instanceof Date) return raw.getTime();
  if (typeof raw === 'number') {
    // Heuristic: values > 1e12 are already ms; smaller are seconds.
    return raw > 1e12 ? raw : raw * 1000;
  }
  const parsed = Date.parse(raw);
  return Number.isFinite(parsed) ? parsed : 0;
}
