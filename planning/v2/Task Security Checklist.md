# Task implementation security checklist

**Status:** 100/100 (10/10 controls verified)
**Verified:** 2026-07-30

Task body checklist edits re-verified 2026-10-03 (#347): the SPA uses the existing
authenticated `tasks.update` body write with optional `expected_body_hash`,
announced by `tasks.bodyPrecondition` in wire 3.10.0. Every checklist tick sends
the SHA-256 digest of the exact original body bytes. The bridge checks it under
the app lock before any task field or timeline write; a mismatch refuses the
whole update with nonretryable `stale_body`. This server check protects edits
made by another device or MCP agent, independently of the SPA's local cache
guards. A refusal conditionally rolls back only the optimistic cached body and
refreshes the task; it never retries the old body. The cached capability controls
availability, and the current greeting gates dispatch of the new parameter.
The hash requires `body`: status/priority/label-only updates carrying it refuse
with `invalid_params` before writing anything.
Bridge tests verify server divergence, no partial writes and exact UTF-8/CRLF
hashing. SPA tests separately cover server/cache divergence and refresh, local
stale reads, newer cache writes, comment immutability and keyboard saving. The
markdown edit replaces only the parsed marker's checked character. Existing
body size and task ownership checks continue to apply.

This checklist covers the security boundary changed by the Task final-repair work. Product/lifecycle completeness is tracked separately from this security score.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | MCP daemon socket is explicitly owner-only (`0600`) | 10/10 | `done_socket_is_explicitly_owner_only` |
| 2 | MCP lifecycle and conversation frames require the current per-session capability | 10/10 | `mcp_control_frames_require_the_current_session_token` |
| 3 | Session capabilities rotate when an agent process is replaced and are never logged | 10/10 | agent-tab spawn path and harness-spec tests |
| 4 | A reused checkout is a registered worktree at the exact canonical managed path | 10/10 | `restore_rejects_an_existing_unregistered_directory` |
| 5 | Reused checkout common-dir, registered name, exact branch, HEAD, and ancestry are verified | 10/10 | `WorktreeManager::verify_existing_worktree` and restore suite |
| 6 | Branch recovery never falls back to the moving base | 10/10 | local/remote original-branch restore tests |
| 7 | Recovery fetch is noninteractive and bounded | 10/10 | `GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=Never`, null stdin, 30-second deadline |
| 8 | Agent-supplied typed references are shape checked and Task-owner scoped; `expected_body_hash` requires `body`, otherwise `invalid_params` | 10/10 | MCP typed-link ownership and canonical-link round-trip tests; `a_task_body_precondition_requires_a_body_and_a_lowercase_sha256_hash` |
| 9 | File references remain traversal fenced and commit references are recorded immutable boundaries | 10/10 | thread-link validation tests and boundary ownership checks |
| 10 | Repository scanners are clean | 10/10 | `semgrep --config auto .` (0 blocking findings); Gitleaks full-history scan (0 leaks) |

**Total: 100/100.**
