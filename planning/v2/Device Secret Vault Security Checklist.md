# Device secret vault foundation security checklist

Scope: #1001's unadvertised bridge storage/lifetime foundation. This does not
qualify sealed uploads, session release authority/audit, raw CLI retrieval,
wire admission or SPA delivery; those remain separate reviewed slices.

**Scoped score: pending gating re-verification.** Ten foundation controls, ten points each.

| # | Control | Evidence |
|---|---|---|
| 1 | Dedicated random device-local key; no identity/transport/MCP key reuse; narrow value/key buffers do not implement ordinary Debug or serialization. | `secrets/model.rs`, `secrets/vault.rs`; value bounds and round-trip tests. Buffers zeroize on drop where supported; no blanket secure-memory promise. |
| 2 | Private owner-only directory/files, no-follow/nonblocking regular-file access, no symlink/FIFO/hardlink intake or caller leaf paths. | `private_permissions_and_no_plaintext_temp_files`, `key_and_blob_symlinks_and_fifos_are_refused_without_blocking`, directory/mode/hardlink and pinned-directory tests. |
| 3 | AES-256-GCM binds every context field, fresh nonce and version/key identifier; tamper/mismatch fails closed. | `encrypted_round_trip_fresh_nonce_and_restart`, `ciphertext_header_nonce_tag_and_body_tamper_fail_closed`, `every_authenticated_context_field_is_bound`, `canonical_project_id_is_authenticated`. |
| 4 | Ciphertext-only temporary writes are fsynced and published atomically without replacing an accepted blob; only confirmed key absence permits durable `key_lost` retirement and new-key creation. Present corrupt keys and every other open failure preserve keys/blobs/grants unchanged. | `existing_blob_is_never_replaced`, `concurrent_publishers_cannot_replace_each_others_blob`, missing/corrupt/wrong/live-key tests; key-loss recovery and failure-injection barrier tests. |
| 5 | Dedicated migrated SQLite metadata contains no value or sealed upload body; loaded expiry metadata is consistent; admission claims are atomic with their operation. | Migration/restart, corrupt-deadline, all-or-nothing attachment and shared-transaction rollback tests; `attach_in` refuses non-FULL outer transactions. |
| 6 | Values are at most 16 KiB and contain no NUL; quotas are 100 staged per creator/device, 1,000 retained blobs per device and four upload/read jobs, without eviction. | Value-limit, quota/preflight and `four_jobs_are_admitted_without_eviction` tests. Metadata preflight precedes encryption and final insertion rechecks quotas atomically. |
| 7 | TTL begins on attach; null means no expiry. Every read checks absolute/monotonic deadlines and persisted clock highwater; observed expiry survives restart and rollback recovery. | TTL-boundary/null/restart tests; external attach phase-change clock tests; frozen-UTC and rollback monotonic-expiry regressions. |
| 8 | Durable denial precedes unlink; cleanup failure never reopens access. Startup/periodic cleanup uses no app lock and removes orphans, expired stages and retired blobs. | `revoke_is_durable_before_failed_unlink_and_sweep_retries`, startup orphan/stage cleanup and `periodic_sweep_needs_no_app_lock_or_client`. |
| 9 | Agent grants retire with reset/removal; Project grants survive their originating conversation and retire on project deletion. Changed device identity cannot reopen old grants. | Reset/removal/owner/project lifetime tests and device-change test; project retirement FULL-commits before external config removal. Release membership checks belong to slice B. |
| 10 | Build backup/export/migration and updater rollback do not carry vault/key files; new overlapping sources are refused, and persisted layout overlaps disable only secret access. Foundation remains unadvertised. | Database backup, 107 targeted migration tests, config/source/updater isolation tests; unchanged RPC/version/capability surface; final Clippy/fmt/Semgrep/Gitleaks checks. |

The focused regressions were observed failing before implementation. Final
integrated commands, exit codes and frozen head are recorded on #1001 with
attached logs. The full suite is reserved for the coordinator's serial merge
gate as instructed.

Database restore intentionally does not restore values/key. Startup remains available. Only confirmed absence retires grants as
`key_lost`; a fresh key can then serve new IDs after that barrier. Present
corrupt keys and every non-NotFound fault preserve all grants/files unchanged,
with secret reads unavailable until repair. Tests cover database-only restore,
short/same-length corruption, modes/ownership/hardlinks, EIO, repair, startup
sweep failures, independent source fences, redacted Debug and expiry reasons.
Unlink is logical deletion, not guaranteed SSD/snapshot erasure, and does not
rotate an upstream credential. Later authorized raw reads intentionally expose
exact values to trusted agents and may enter their tool output/transcripts.

## Earlier foundation validation — 2026-11-04

All gates ran under `nice -n 10`, with task scratch, `CARGO_TARGET_DIR` and
`TMPDIR` outside the checkout. Process exits, not output summaries, determined
success:

| Gate | Result | Exit |
|---|---|---|
| `cargo test --lib secrets` | 65 passed, no failures or ignores | 0 |
| Cargo-built lib test binary, `config::vault_tests` | 4 passed, no ignores | 0 |
| Cargo-built lib test binary, `update::installer::tests::vault_paths` | 2 passed, no ignores | 0 |
| `cargo test --test dev_channel` | 24 passed, no ignores | 0 |
| Targeted migration Python modules | 107 passed, no skips | 0 |
| Ruff on changed Python files | No findings | 0 |
| `cargo clippy --all-targets -- -D warnings` | No warnings, no new ratchet annotations | 0 |
| `cargo fmt --check` | Clean | 0 |
| Semgrep `--config auto --error` on changed files | 50 files, 293 rules, no findings | 0 |
| Gitleaks task commit range | No leaks; repeated before final push | 0 |
| Diff whitespace check | Clean | 0 |

The recovery regression was first observed failing against the actual built
bridge library: a real database backup restored into a new AppState refused
startup. The revised implementation passes that case and both short and
same-length corrupt-key cases. SQL-trigger failures at retirement and key-ID
clearing leave the existing key untouched and secret access unavailable. The
standalone filesystem tests also reject unsafe recovery paths and prove failed
cleanup cannot replace a key. Final head and complete changed-file list are on
#1001; the full suite remains the serial merge gate's responsibility.

## Review follow-up — rollback exclusion

The quick review found that rollback could move a newly created target vault
into a `.retired-*` artifact or restore a vault-bearing backup. Both trees are
now refused before rollback traversal, and the guard repeats at restore
admission before any shared-remote edits or directory renames.

Sixteen new tests failed before the fix (16 failed, 9 passed; exit 1). They
cover target and backup directories, regular files, FIFOs, broken symlinks and
entries inserted after preflight. They verify unchanged trees, manifests and
synthetic remotes, no traversal and no retired artifact. The final five targeted
migration/Git/CLI modules pass all 107 tests with no skips (exit 0); changed
Python Ruff and diff checks pass (exit 0). Rust source is unchanged from the
previous frozen review head, retaining its 95 passing targeted tests and clean
Clippy/format checks. Final Semgrep/Gitleaks are repeated for the revised head.

## Gating review follow-up — seven findings

This revision narrows recovery to independently confirmed key absence. Corrupt
present keys and all other errors preserve the complete key/blob/grant state.
Layout conflicts and startup sweep failures disable only secrets; new-source
admission retains its fence even without a service. Metadata Debug redacts names
and arbitrary nested strings, and expiry uses its own durable `expired` reason.
The actual diagnostic emitter is pinned to a count-only format. The migration
README now gives a forward-fix/fresh-state operator path after first startup
makes rollback unavailable, without moving or deleting the vault.

Each behavior regression was observed failing before implementation. Final
counts and gates for this revision will be recorded below and on #1001.
