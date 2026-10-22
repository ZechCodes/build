# Durable write wire fixtures (3.24.0)

These fixtures define the journaled write envelopes before any semantic RPC is
admitted through them. The internal `ACK_CONFIRM_METHODS` list is empty. The
greeting advertises `bridge.epoch` and the real `writes.status` and
`writes.subscribe` methods. Hello also exposes `ack_confirm_methods: []`,
mirroring the internal list; this is a greeting field, not a capability name.
There is no `writes.ackConfirm` feature or `write.mode` opt-in. Clients use
ack/confirm envelopes only for methods in the current greeting's
`ack_confirm_methods` list.

- `request.json` supplies a stable UUID operation ID and explicit dependencies.
  Omitting `after` requests bridge-side inference; an empty array is an explicit
  override and still must include every required creator dependency.
- `ack.json` is durable acceptance, distinct from `{id, accepted: true}` transport
  receipts. Its result restores the accepted cache overlay and reserved IDs.
- `confirmed-success.json` includes the complete existing `tasks.comment` result,
  owner publication fences, and precise refresh addresses. Each owner fence includes
  its generation: sequence order applies only within that generation.
- `confirmed-failure.json` names the effect outcome so an unapplied failure can
  remove only its own accepted overlay.
- `status.json` returns authorized saved operation summaries for at most 128 IDs.
  Results are capped at 512 KiB with a stable `next_cursor` when necessary.
- `subscribe.json` atomically binds the same bounded ID set, its current states,
  journal incarnation, and a decimal string outbox cursor. Later confirmations
  are delivered at least once, independently of ordinary change coalescing.
- `epoch-greeting.json` shows the epoch/incarnation fields and relevant capability
  subset. All authenticated application envelopes carry the process epoch.

The example comment is illustrative. No method is enabled for early acceptance
in this slice. The status and subscribe fixtures live here because they share
these types; their served-method fixture entries also live at the v1 root.

For clients implementing wire 3.24.0:

- Any request carrying `write` metadata is currently refused with nonretryable
  `unavailable`; no semantic method is enabled yet. Keep the intent and draft,
  and do not interpret this refusal as durable acceptance.
- `writes.subscribe` replaces the caller's entire subscribed operation set.
  Send the whole selected outstanding ID set on every call, including IDs
  retained from earlier subscriptions. Confirmations are sent only to authorized
  subscribers. The bound remains 128 IDs: if more operations are outstanding,
  subscribe the oldest 128 as one complete replacement set, track the rest with
  paged `writes.status` calls in batches of at most 128 IDs, and refill the
  subscribed set as operations reach a terminal state.
- Subscriptions belong to the session opening and carrier. Re-subscribe and
  query status whenever either changes, even if the bridge epoch stays the same.
- The subscribe `cursor` reports the atomic outbox position for that snapshot;
  it is informational and is not a replay parameter. If a subscribe or status
  response has `next_cursor`, continue with `writes.status`, passing that value
  as `cursor` and keeping the same complete, ordered `operation_ids` list.
- `status.json` and the root `writes.status.json` cover accepted, executing,
  succeeded, failed, not-found, and expired-result summaries. Their two examples
  show the first page and its continuation. Actual page boundaries depend on
  encoded bytes; the sample bodies are small enough to read without padding.
