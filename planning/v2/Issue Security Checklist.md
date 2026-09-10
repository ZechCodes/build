# Issue implementation security checklist

**Status:** 100/100 (10/10 controls verified)
**Verified:** 2026-07-30

This checklist covers the security boundary changed by the Issue final-repair work. Product/lifecycle completeness is tracked separately from this security score.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | MCP daemon socket is explicitly owner-only (`0600`) | 10/10 | `done_socket_is_explicitly_owner_only` |
| 2 | MCP lifecycle and conversation frames require the current per-session capability | 10/10 | `mcp_control_frames_require_the_current_session_token` |
| 3 | Session capabilities rotate when an agent process is replaced and are never logged | 10/10 | agent-tab spawn path and harness-spec tests |
| 4 | A reused checkout is a registered worktree at the exact canonical managed path | 10/10 | `restore_rejects_an_existing_unregistered_directory` |
| 5 | Reused checkout common-dir, registered name, exact branch, HEAD, and ancestry are verified | 10/10 | `WorktreeManager::verify_existing_worktree` and restore suite |
| 6 | Branch recovery never falls back to the moving base | 10/10 | local/remote original-branch restore tests |
| 7 | Recovery fetch is noninteractive and bounded | 10/10 | `GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=Never`, null stdin, 30-second deadline |
| 8 | Agent-supplied typed references are shape checked and Issue-owner scoped | 10/10 | MCP typed-link ownership and canonical-link round-trip tests |
| 9 | File references remain traversal fenced and commit references are recorded immutable boundaries | 10/10 | thread-link validation tests and boundary ownership checks |
| 10 | Repository scanners are clean | 10/10 | `semgrep --config auto .` (0 blocking findings); Gitleaks full-history scan (0 leaks) |

**Total: 100/100.**
