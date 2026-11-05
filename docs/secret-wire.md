# Secret attachment wire (3.36.0)

Secret uploads are accepted only through an authenticated device-user opening.
HTTP and WebRTC share the same typed methods and execution authority. The
creator binding is the bridge's single-owner principal, `device_user`, rather
than a transient opening ID. The device ID must match the local vault.

The three feature names (`secrets.attachments`, `secrets.sealedUpload`, and
`secrets.metadataEvents`) and four methods are declared but withheld from the
session greeting. The UI enablement slice must explicitly advertise them only
when the installed CLI and composer are ready. Existing text/file messages
keep their contract.

## Upload and reconciliation

`secrets.create` (`POST /v1/secrets`) accepts `request_id` (canonical UUID),
`device_id`, `creator`, `scope` (StageContext), `grant` (GrantScope), `name`,
nullable `ttl_seconds`, and `sealed_payload`. Unknown fields are refused. The
entire request is capped at 64 KiB; the sealed inner frame is exactly 32 KiB.
The value is UTF-8 encoded and represented as standard unpadded base64 inside
the encrypted envelope, so even 16 KiB of JSON control characters fits without
changing the padding size. `@build/secure-transport` exports `sealSecretUpload` to construct this frame
before any generic adapter sees it. It uses the authenticated device transport
public key and binds every outer field inside the versioned sealed box.

StageContext contains `owner_id`, `conversation_id`, `generation`, and nullable
`draft_id`. An existing agent uses its canonical conversation ID and immutable
thread ID as generation. A provisional recipient uses a canonical draft UUID,
the already-known final workspace/project owner, and nonempty provisional
conversation/generation handles. It cannot transfer to a different owner.
GrantScope is `{scope:"agent",agent_id,conversation_id,generation}` or
`{scope:"project",project_id}`. Project IDs are canonical registry IDs.

The response is metadata only. Repeating the same request and exact sealed
bytes returns the accepted record; changing either bound context or ciphertext
is a conflict. Do not persist the sealed upload body or a plaintext value in a
write queue, cache, draft, history, or diagnostic. After timeout/reload, use
`secrets.list` (`POST /v1/secret-queries`) with `request_ids` to reconcile.

List scope is `{project_id,stage?}`. With no stage, it returns project grants
independently of their originating conversation. With a stage, it also returns
agent grants for that exact stage/final recipient. Optional `secret_ids` and
`request_ids` each contain at most 100 selectors. `limit` is 1–100; opaque
`after` cursors are bound to the query. Responses contain `secrets`,
`server_now`, and nullable `next`. Every descriptor has only IDs, name, grant,
stage, TTL, timestamps, revision and status; no blob state, digest or value.

`secrets.revoke` (`POST /v1/secrets/{secret_id}/revocations`) accepts scope,
secret ID and a UUID operation ID. Revocation commits denial and a metadata-only
receipt together before unlink. The operation ID is bound to the device, creator,
secret ID and exact scope: retries return the original accepted response, and
reuse with a different secret or scope conflicts. Exact responses are retained
for at most 90 days and the newest 4,096 revocation operations across the store.
Older receipts are pruned during maintenance or evicted to keep revocation
available; the permanent tombstone still denies release after a receipt expires.
An already-expired secret remains expired, including when its expiry has not
yet been swept.
`secrets.audit` (`POST /v1/secrets/{secret_id}/audit-queries`)
accepts scope, secret ID, nonnegative `after` sequence and limit 1–100. It
returns metadata events, server time and nullable next sequence. List/audit
execute in the shared bounded SQL lane.

## Send and delivery

`thread.post` accepts optional `secret_ids` (at most ten distinct IDs) and a
`draft_id` for a provisional draft. Secret-bearing posts require authenticated
user authority, an operation ID and the exact current thread ID. Agent/tool
posts and bare provider commands cannot bind secrets. Send checks the complete
set, current recipient/generation and distinct names. A FULL SQLite transaction
commits all bindings, the message, frozen descriptors and the operation receipt
together. A replay returns that original receipt and never restarts TTL.

TTL starts at durable acceptance. Null TTL leaves `expires_at` null. The
immutable encrypted stage context is retained; a separate final-binding row
records the exact recipient when a draft is promoted. Metadata reads can
reconcile by the original draft or read the final recipient, while release
checks use the current committed final binding. The delivered metadata block
comes from `bridge/templates/messages/attached_secrets.md`, wrapped by each
harness. It includes the grant scope, never the value. Raw CLI retrieval is an
intentional disclosure boundary; see [release authority](device-secret-release.md).

`secrets.changed` carries IDs, revision and scope context only. Project events
route by canonical project ID without looking up the originating conversation.
Treat it as an invalidation: bounded list reads reconcile loss/reordering, and
revisions prevent stale metadata from becoming authoritative. There is no
second durable event cursor and no web-push secret content.

A coalesced hint names at most 100 secret IDs; `truncated:true` with no IDs
requires a full refresh of that scope. If an inactive subscription accumulates
more than 1,000 distinct scopes, it receives a bounded `scope:"device"` hint
with `truncated:true` and no IDs. Refresh every held secret-metadata scope for
that device. This fallback remains restricted to subscriptions requesting the
`secrets.changed` kind. Events are hints, so compare revisions on refreshed
individual descriptors; a different secret's higher revision cannot suppress
another secret's invalidation.

Lifecycle audit facts (`created`, `attached`, `expired`, `revoked`) and existing
release/denial facts share one monotonically increasing audit sequence. The
schema-25 upgrade extends the old audit outcome constraint using a savepoint
and table rebuild, preserving every retained row and the AUTOINCREMENT
high-water even when the highest old row was pruned. Lifecycle facts commit
with their metadata mutation. Release/denial summaries retain their original
meaning; creating or revoking a secret does not increment a denial count.

## Schema compatibility

Store schema remains 25, extending the unadvertised foundation with final-binding
metadata and idempotent `ensure_lifecycle_audit` / `ensure_revocation_receipts`
upgrades within the shared `secret_audit_upgrade` savepoint. The audit CHECK is
rebuilt from the complete known outcome set: `released`, `secret_unavailable`,
`secret_expired`, `secret_revoked`, `created`, `attached`, `expired`, `revoked`.
The savepoint upgrade adds CLI audit grouping columns before rebuilding the
CHECK, copies all 14 columns by name, and derives the table and five indexes
from the canonical schema. Both fresh and ALTER-upgraded CLI column orders
preserve their data and converge on the same schema. Audit rows, sequence IDs
and SQLite's deleted-row AUTOINCREMENT high-water survive repeated initialization.
Lifecycle appends set attempt count to one and last occurrence to the fact's
timestamp. Only `secret_unavailable`, `secret_expired` and `secret_revoked` enter
denial grouping and its 4,096-group cap. Retention cleanup excludes lifecycle
facts. Internal release/denial readers exclude those facts; public audit pages
include them. Release/denial authorization and summary accounting retain their
existing helper.

If secret invalidation contexts exceed their bounded coalescing budget, a
`scope:"device",truncated:true,secret_ids:[]` hint requests a full metadata
refresh for existing secrets subscribers. Per-secret revisions are metadata
versions, not a single scope-wide event cursor: do not discard one secret's
invalidation merely because another secret has a higher revision.
