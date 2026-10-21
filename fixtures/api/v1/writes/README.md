# Durable write wire fixtures (3.24.0)

These fixtures define the journaled write envelopes before any semantic RPC is
admitted through them. The internal `ACK_CONFIRM_METHODS` list is empty. The
greeting advertises `bridge.epoch` and the real `writes.status` and
`writes.subscribe` methods; it does not advertise the internal list or a
`writes.ackConfirm` feature. Requests have no `write.mode` opt-in.

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
