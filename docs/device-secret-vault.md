# Device secret vault foundation

This is internal storage infrastructure, with no advertised capability, RPC,
HTTP route or CLI. Upload authentication, process authority, audit and message
admission are separate slices. Do not expose the raw storage read method before
those checks are implemented. Values are never ordinary file attachments.

The vault is `<BridgeConfig.state_root>/secrets/`, outside project/worktree
storage. Startup and project/source admission reject tree roots inside or
containing the vault, resolving symlinks and missing leaves in both channels.
Production overrides that use the entire `~/.build` directory as a task,
project or worktree root must instead use a disjoint subdirectory. Persisted
updater jobs are checked before rollback backup/install/recovery too.
The directory is private (0700); key and encrypted files are private
regular files (0600), accessed relative to a pinned directory descriptor with
no-follow checks. The dedicated random 256-bit `vault.key` is independent of
identity, transport and MCP keys. Missing, corrupt, insecure or substituted key
files fail closed; existing state never triggers silent key regeneration.

Each `sec_<canonical UUID>` file has a versioned header, vault key identifier,
fresh random nonce, AES-256-GCM ciphertext and tag. A domain-separated canonical
metadata encoding authenticates device, secret ID, creator, name, TTL, grant
scope and immutable staging context. Only ciphertext reaches temporary files;
private temporary creation, fsync, atomic publication and directory fsync make
the blob durable before metadata acceptance. Values/keys have no ordinary Debug
or serialization implementation; narrowly scoped buffers are zeroized where
supported. This does not promise secure memory against an administrator.

Metadata alone lives in dedicated SQLite tables, using the existing writer.
A create accepts at most 16 KiB with no NUL, at most 100 staged values per
creator/device and 1,000 retained blobs per device. Four upload/read jobs can
be admitted concurrently; quota refusal never evicts a live secret. A failed
metadata commit leaves an inaccessible orphan for cleanup. Upload UUID and
ciphertext digest are reconciliation metadata, never a hash of plaintext.

A stage lasts at most 24 hours. Attaching starts the optional TTL at durable
message acceptance. A null expiry means no expiry; reads remain repeatable.
Every read checks its tombstone, device, absolute expiry (`now >= expires_at`)
and the process monotonic deadline. A persistent wall-clock high-water mark
survives restart; clock rollback denies TTL-bearing reads until wall time
catches up, and detected monotonic expiry is durably retired before denial, so restart or
clock recovery cannot reopen it. No-expiry
attached values remain usable during rollback. Restoring the entire disk and
clock against an administrator is out of scope.

Agent grants bind an agent, canonical conversation and generation. Reset or
removal retires them. Project grants bind canonical project identity, survive
originating conversation reset/removal, and retire on project deletion. A
changed device identity cannot reopen old grants. The authorization slice must
also validate current committed agent membership on every release.

Revoke commits a durable tombstone before attempting unlink. Startup and
periodic cleanup run without the app lock and remove orphaned, revoked,
expired and staged-expired blobs; failed unlinks retain denial and are retried.
The cleanup service serializes its own create/read/delete work to avoid deleting
a blob between encryption and metadata commit. Metadata retirement hooks do no
filesystem work. Cleanup forces a durable SQL boundary before physical deletion.

The same-main-store transaction hook `store::secrets::attach_in` is required
for future message admission. The convenience storage attach method is for
internal composition/tests; calling it and posting a message in two independent
commits is not a valid secret-bearing send.

Build's database backup/export copies metadata only, excluding vault files and
the key. In this foundation, daemon construction propagates vault-open failure:
a restored database containing secret records requires explicit vault recovery
or retirement before startup; it does not generate a new key. A later recovery
interface can keep unrelated history available without enabling value reads. State migration excludes source vaults and refuses any existing target
vault even with `--force`, so it cannot copy a key into staging or a rollback
backup. Restored metadata can refer to unavailable values. User-created disk
snapshots are outside Build's exclusion. Unlink is logical deletion, not promised
forensic erasure on SSDs; revoking a vault entry does not rotate an upstream
credential. Authorized raw retrieval in later slices intentionally exposes the
exact value to the trusted agent and may enter its transcript or tool output.
