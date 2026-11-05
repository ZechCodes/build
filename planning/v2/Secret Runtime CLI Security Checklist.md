# Secret runtime CLI security checklist

Scope: #1004 on origin/dev 3e6e81bd9. This covers runtime retrieval,
installation and launch environments, plus the #1002 denial/audit follow-ups.
HTTP routes, uploads, wire resources and final-recipient scope binding belong
to #1003 and are not changed here.

**Scoped score: 100/100.** All controls below passed targeted validation.

| # | Control | Evidence |
|---|---|---|
| 1 | Retrieve through the private issued-session socket, without keys, daemon state, CWD or identity discovery. | `secret_cli.rs`; missing/stale context and invalid daemon-config regression. |
| 2 | Explicit facade environment is mandatory and must match the launch channel. | Shared runtime/management facade contract tests; no registry or production management bypass. |
| 3 | Exact stdout bytes, no newline, bounded value; fixed bounded stderr errors. | Whitespace/multiline/16 KiB, truncated payload, oversized header/body, untrusted error and non-UTF-8 argv regressions. |
| 4 | No value or capability in argv, audit or incidental credential files. | Shared facade/socket tests, durable audit identifier regressions, profile argv assertions and provider qualification. Intentional stdout/transcript capture is an accepted boundary. |
| 5 | Every harness receives its own shell capability; a recognized private runtime directory is added only to agent PATH. | Actual PTY, ADK and app-server carrier shell fixtures; all-provider spec matrix; prepared launch test. |
| 6 | Codex preserves the effective user policy, including inheritance, custom settings and filters; only six dedicated variables are overlaid, with no PATH set override. | Narrow policy test, filtered shell-child test and real Codex 0.166.0 shell calls against a loopback fake provider. MCP auth, other Bridge variables and AWS credentials remain absent. |
| 7 | Known TUI capability-on-disk exposure is restricted and cleaned through exact registered identities. | 0600 exclusive publication; byte-identical user config, held-handle session cleanup, private bounded inode registry and no user-home enumeration. Symlinked homes and unavailable probes degrade to ordinary launches without secret authority. Startup cleanup runs off runtime workers. |
| 8 | App-server private thread config does not enter default provider persistence in the qualified version. | Completed initial and restarted/resumed turns with rotated capabilities; rollout/default-log scans found zero copies. TUI profile-selected exec likewise left no copy outside its expected profile. |
| 9 | Child agents, probes and plain terminal tabs acquire no inherited secret authority. | Prefix removal through each process carrier, terminal and installed probe tests; independent session rotation/reset authority regressions. |
| 10 | Private transport and denial persistence have bounded resource use, while releases remain individually auditable. | 250-ms pre-auth deadline, minute/session denial coalescing, 4,096-group cap, additive migration and indexed bounded caller reader; existing rate/concurrency/audit-before-release tests. |

The installer, deployment/rollback and self-update ship the checkout-independent
launcher without overwriting an unrelated management CLI. Broken stdout and
termination leave no value files. All runtime authority is for trusted Build
agents: same-user/root access, arbitrary provider debug instrumentation and an
agent intentionally saving/printing its own authority or retrieved values are
outside this boundary. See `docs/device-secret-release.md` for exposure details.

The final #1004 report retains the frozen head, actual command exit codes and
provider qualification evidence. The full repository suite remains the serial
merge gate; targeted checks skip nothing.


## Initial validation — 2026-11-05, head 5b09aa024

All commands ran under `nice -n 10`; TMPDIR and CARGO_TARGET_DIR remained under
`/home/zech/Projects/.agent-scratch/secrets-d/`. No tests were skipped.

- Cargo-built library: `secret` filter, 164 passed, exit 0.
- `cargo test --test secret_cli -- --test-threads=1`: 7 passed, exit 0.
- Cargo-built library: updater installer 24, runtime launcher 2, prepared launch
  1; every command exited 0. Total distinct focused Rust checks: 198.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`: exit 0.
- Tools buildctl offline pytest: 346 passed and 13 subtests, exit 0, with the
  opt-in snapshot rehearsal supplied a synthetic scratch fixture (zero skips).
- Tools ruff, changed-shell Shellcheck and 25 installer fixture cases: exit 0.
- Semgrep on all changed Rust/Python/shell paths: zero findings, exit 0.
- Actual Codex 0.166.0 completed-turn, fresh-capability resume and profile
  qualification: passed; no capability in rollout/default logs, and TUI's only
  expected on-disk copy disappeared with profile removal.

The initial all-harness test caught a fixture that incorrectly put Pi state
inside its checkout. The corrected fixture separates those roots; its full
matrix and the complete targeted secret group pass. No complexity ratchet
annotations were added. Gitleaks and diff checks gate the final branch push.


## Review revision — launcher integration and effective user policy

Terra and Opus findings are covered by regressions before the fixes:
legacy rollback/deploy/pinned binaries, occupied runtime destinations, missing
Codex home and home symlinks, user-policy inheritance/filter replacement, and
40-byte stdout writes to closed pipes or `/dev/full`. These reproduced the
reported failures. No new complexity ratchet annotations were added.

The runtime is now `<bin>/.build-runtime/buildctl`, resolving `../build-bridge`.
Global BuildKit/management commands stay untouched. Foreign runtime destinations
are skipped with a warning and never added to agent PATH. The shell installer
prepares its embedded launcher and provenance before replacing the binary;
helper failure preserves the old installation, and failed provenance publication
restores it (or removes a fresh candidate). Historical binaries without the
launcher subcommand still support rollback, old-ref deployment and pinned install.

A bounded, authority-stripped native Codex config probe resolves the effective
user policy before overlay. Both carriers preserve inheritance, custom set and
legacy/modern filters, extending only the six dedicated names. Arbitrary user
keys are safely TOML-serialized; the existing TOML display feature adds locked
`toml_write` 0.1.2. Profile creation uses an owned no-follow directory descriptor.
The delivery path itself is tested, including TUI-only profile publication,
probe authority stripping, unchanged user config and clean checkout status.

Current targeted functional checks: secret filter170, CLI8, private runtime7,
updater25 and prepared launch1 — **211 distinct Rust tests**, all exit0. Tools
pytest351 +13subtests (synthetic rehearsal, zero skips), installer32cases, ruff
and Shellcheck pass. The final task report records clippy/fmt/security-scan and
push results after the fixture complexity refactor.

Four actual Codex 0.166.0 checks cover legacy/modern filters across both carriers.
They validate completed command output and exit0, replacing the earlier weaker
text-search assertion. The user policy remains intact; capability scans find
only the expected live profile, and no copy after cleanup. The reproducible
updated driver and evidence are attached to #1004. The early locale-based
native red check is superseded; the two Rust inheritance failures are the valid
policy red baseline. Initial persistence scans remain valid.

Legacy migration coverage now seeds4100 denials and proves only the newest4096
survive, while releases/totals and timestamp/counter backfills remain correct.
The production audit schema/predicates and #1003 reconciliation are unchanged.


## Delta revision — optional setup and exact cleanup registry

The fatal-setup regression and symlink-target sweep regression failed before
the fixes. Codex secret setup is now optional: home symlinks, legacy profiles,
unsupported/erroring config probes and probe timeouts launch the ordinary
carrier with no secret overlay, secret environment or injected runtime PATH.
Each failure emits one fixed reason warning without diagnostic values.

Build enumerates only its private 0700 registry, bounded to 4096 records of at
most 16 KiB. It durably records the exact profile and parent dev/ino before
writing capability bytes or publishing the final entry. Session cleanup uses
the held directory handle. Startup resolves the recorded user home normally,
checks parent and no-follow entry identities, and removes only exact matches.
Missing entries, user replacements, swapped link targets and inaccessible homes
retire metadata without deleting user files. Live locks preserve active
profiles. The user's CODEX_HOME is never enumerated or pattern-cleaned.

No production audit, scope-check, schema or migration change is included in
this revision; #1003 retains that merge reconciliation.

Validation: Cargo-built serial secret filter191 (including registry15 and the
12 degraded-carrier scenarios), CLI8, runtime7, updater25 and prepared-launch1:
**232 distinct Rust tests**, zero ignored, all exit0. All-target clippy with
warnings denied, fmt, changed-file Semgrep (51 rules/15 files), gitleaks and
diff checks passed. The earlier Python351 +13subtests and installer32-case
qualification is unchanged; this revision changes neither implementation.

An exploratory parallel secret run reported187 passing/four failing: three
fixture registry lock acquisitions and one inherited-carrier output assertion.
A fork/flock reproduction confirms a pre-exec child can temporarily retain
CLOEXEC lock descriptors after the parent's close. The serial run passes all
191 with no skips. Runtime lock contention safely disables optional authority
or delays stale cleanup; it does not block Codex startup. The parallel run is
retained as evidence, rather than counted as passing.


## Parallel-admission follow-up

The previous serial-only qualification was insufficient for the default-parallel
gate. Admission now holds an in-process mutex through publication, waits up to
one second for cross-process flock contention, and explicitly unlocks before
closing the close-on-exec descriptor. Owned profile/metadata acquisition uses a
separate process mutex and the same bounded waiting; sweep liveness checks stay
nonblocking. The retained-descriptor, concurrent-wait and bounded-timeout
regressions failed on 0ecf81024 and pass with this implementation.

Carrier reports now include a completion sentinel; the reader waits for the
sentinel rather than accepting an empty or partial file. Its regression failed
against the old reader. Eight simultaneous Codex launches share one registry
and verify the expected capability in each actual fixture process. The paused
fork regression runs in an isolated child so it does not retain other tests'
leases. No tests are removed or ignored.

Cleanup checks the exact inode immediately before unlinkat via the held parent
fd. The remaining check-to-unlink replacement window is explicitly accepted
within the same-user trust boundary; Linux has no conditional unlink-by-inode.
Pre-check replacements remain preserved. No quarantine rename is performed.
See `docs/device-secret-release.md` and the coordinator's decision on #1004.

Final follow-up qualification: five consecutive default-parallel runs each
passed196/196, zero failed/ignored (980 executions, including all original191
cases plus five new regressions). RUST_TEST_THREADS was unset and no thread
count option was supplied. Each run includes eight simultaneous actual Codex
fixture spawns with the expected capability. All-target clippy-Dwarnings, fmt,
Semgrep51rules/5changedRustfiles and diff checks passed. TDD red/green and the
five complete run logs accompany the task evidence. The previous parallel
caveat is superseded by this proof. Commands ran nice-n10 with scratch temp and
Cargo target directories; gitleaks gates the single follow-up push.
