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
| 5 | Every harness receives its own shell capability and absolute runtime PATH. | Actual PTY, ADK and app-server carrier shell fixtures; all-provider spec matrix; prepared launch test. |
| 6 | Codex keeps default credential filtering; only dedicated secret variables and PATH are restored. | Narrow policy test, filtered shell-child test and real Codex 0.166.0 shell calls against a loopback fake provider. MCP auth, other Bridge variables and AWS credentials remain absent. |
| 7 | Known TUI capability-on-disk exposure is restricted and cleaned. | 0600 exclusive profile creation; user config byte identity, session-end/failed-spawn removal, owned canonical-prefix orphan sweep, symlink refusal and live-file locks. Startup calls the sweep off runtime workers. |
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


## Validation — 2026-11-05

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
