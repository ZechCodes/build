# Pi TUI harness security checklist

**Status:** 100/100 (10/10 controls verified)
**Verified:** 2026-09-03

This checklist covers the Pi TUI harness, its Build-owned extension and MCP child, session identity, and provider boundary. Product completeness and upstream Pi compatibility are tracked separately from this security score.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | Private Pi state is outside and non-overlapping with each Pi launch's canonical checkout | 10/10 | `ensure_pi_root_outside_checkout`; `pi_spec_rejects_state_inside_checkout_before_creating_pi_files`; `pi_spec_rejects_checkout_inside_pi_root_before_creating_pi_files` |
| 2 | Private directories are `0700`; the content-verified extension is atomically published as `0400` | 10/10 | `ExtensionMaterializer`; `extension_install_is_idempotent_private_and_repairs_content`; `extension_is_private_when_atomically_published`; `publication_failures_leave_only_complete_private_results` |
| 3 | The MCP child uses a mandatory absolute executable and direct argv with no shell | 10/10 | `HarnessContext::resolved`; `runtimeConfiguration`; `McpStdioClient.start` (`spawn(command, ["mcp", "--task", owner], { shell: false })`); `resolved_context_uses_the_canonical_bridge_and_configured_state_root`; `extension_discovers_tools_forwards_calls_and_converts_only_text` |
| 4 | Only the current rotated session token is accepted; stale and wrong tokens are rejected | 10/10 | `AgentSpawnReservation::claim`; `authenticated_mcp_owner`; `mcp_control_frames_require_the_current_session_token`; `authenticated_listener_rejects_rotated_and_wrong_tokens_and_keeps_canonical_errors`; `extension_uses_real_mcp_stdio_token_and_canonical_tool_errors` |
| 5 | Pi discovers and forwards only the existing MCP stdio tools, schemas, and daemon-side domain validation | 10/10 | `McpStdioClient.initialize`; `buildTools`; `DoneServer`; `tools_list_exposes_thread_tools_and_done`; `extension_discovers_tools_forwards_calls_and_converts_only_text`; `extension_uses_real_mcp_stdio_token_and_canonical_tool_errors` |
| 6 | Framed JSON-RPC strictly correlates pending ids and accepts exactly one validated result or error | 10/10 | `parseResponseEnvelope`; `McpStdioClient.readChunk`; `terminal_protocol_failures_terminate_pi`; `invalid_error_rejects_every_pending_and_future_call_with_one_latched_failure`; `framing_and_utf8_failures_are_terminal`; `write_failure_is_latched_and_terminates_pi` |
| 7 | Shutdown escalation timers are bounded; killed MCP children are awaited and reaped, and Pi never continues after tool discovery or transport fails | 10/10 | `McpStdioClient.stopChild` (stdin close, bounded wait, `SIGTERM`, bounded wait, `SIGKILL`, then await); `empty_discovered_tool_set_aborts_pi_and_closes_and_reaps_the_mcp_child`; `extension_kills_a_child_that_refuses_normal_close`; `registration_failures_abort_pi_and_close_and_reap_the_mcp_child`; `terminal_protocol_failures_terminate_pi`; `pi_mcp_child_death_runs_the_normal_tab_exit_path`; `a_harness_spec_error_releases_the_reservation_and_never_spawns` |
| 8 | Pi binds exact Build agent identity across spawn and resume and refuses session switching or forks | 10/10 | `PiHarness::spec`; `pi_respawns_the_same_agent_exactly_and_separates_another_agent`; `pi_launch_identity_reaches_the_session_through_tab_spawn`; `extension_discovers_tools_forwards_calls_and_converts_only_text` (`switchRefused`, `forkRefused`) |
| 9 | Pi is coding-only and is rejected at router configuration and construction boundaries | 10/10 | `validate_router_choice`; `pi_is_rejected_as_a_router_while_existing_providers_remain_valid`; `router_session_construction_rejects_pi_again`; `configured_pi_router_is_rejected_and_the_default_stays_claude_adk` |
| 10 | Full tests and repository security scanners are clean | 10/10 | From the repository root: `(cd bridge && cargo test)` (1,501 passed, 7 ignored) and `(cd spa && npm test)` (2,819 passed); tracked-file `semgrep --config auto .` plus a separate explicit scan of the five untracked Pi implementation/test files (0 findings); full-history `gitleaks git .` and current-tree `gitleaks dir .` (0 leaks) |

**Total: 100/100.**

Outside the score: fake-Pi tests verify the PTY and extension contracts without a vendor model call; compatibility with a user's installed Pi version and environment remains an environmental verification. Pi runs as the user, and this checklist makes no sandboxing claim.
