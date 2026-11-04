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
files fail closed. The metadata stores the active key identifier, so a changed
32-byte key also fails closed. Startup durably retires affected grants with
`key_lost`, preserving normal bridge operation. Only after every old grant is
retired may explicit cleanup create/register a fresh random key; old contexts
remain unavailable. An unknown key is never adopted, even after interrupted
recovery with zero lifetime records. A single bounded startup diagnostic gives
only the affected count. If retirement or safe cleanup fails, secret access
stays unavailable and the key is not substituted for unretired grants.

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
the key. Restoring metadata without the vault starts the daemon, retires old
records as `key_lost` and makes old IDs return the same unavailable error. A
new key can serve new secrets after that durable retirement; it never serves
the old grants. The same policy covers corrupt regular keys. Unsafe filesystem
entries or failed retirement keep secret access unavailable while the rest of
the bridge remains usable. State migration excludes source vaults and refuses
any existing target vault even with `--force`, so it cannot copy a key into
staging or a rollback backup. Rollback refuses a vault in both the current
target and the retained backup before walking either tree. It checks again at
restore admission before changing remotes or renaming state, so a vault created
after preflight cannot be retained in a `.retired-*` artifact or restored from
a backup. Restored metadata can refer to unavailable values. User-created disk
snapshots are outside Build's exclusion. Unlink is logical deletion, not promised
forensic erasure on SSDs; revoking a vault entry does not rotate an upstream
credential. Authorized raw retrieval in later slices intentionally exposes the
exact value to the trusted agent and may enter its transcript or tool output.
