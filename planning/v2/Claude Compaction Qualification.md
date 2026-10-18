# Claude compaction qualification — #656

Observed 2026-10-18. Native enforcement remains **unqualified for production**
on both Claude carriers. The translator implements the inspected 2.1.284
formula, but a version and requested model do not establish effective output
budgets or the model used after an in-turn native fallback.

## Isolated execution

Direct executable: `/home/zech/.local/share/mise/installs/claude/2.1.284/claude`.
SHA256: `5cd90aabd83f8a15136c35aa37bb1d92b348993573316643dc3fe4e04afbf88f`.
`--version` reported `2.1.284 (Claude Code)` (exit 0).

Four small model runs used a throwaway HOME, CLAUDE_CONFIG_DIR and cwd beneath
`/tmp/build-656-qualification`. The authorised credential file was copied once,
chmod 0400 and bind-mounted read-only. Its original was never modified and the
copy was deleted after the four runs. No login/logout/status, installer, bridge
binary, browser, pairing or network/firewall configuration was used.

All runs executed under `nice -n 10` in a constructed environment. Ordinary
settings sources and built-in tools were disabled. A strict synthetic stdio MCP
server exposed one tool returning numbered text; each prompt requested one
call followed by `OK`. Claude truncated the tool result, so its full generated
text was not all retained in context. Each fresh session used a generated UUID;
the next run resumed that exact UUID. The command payload was:

```text
-p --verbose --output-format stream-json --model claude-haiku-4-5
--setting-sources '' --settings <inline-json> --autocompact 100000
--mcp-config <synthetic-server-json> --strict-mcp-config --tools ''
--dangerously-skip-permissions --max-turns 3
[--session-id <uuid> | --resume <uuid>] <synthetic-tool-prompt>
```

The inline settings merged a no-op PreCompact hook, `autoCompactEnabled:true`,
`precomputeCompactionEnabled:false`, `autoCompactWindow:100000` and env:
`CLAUDE_CODE_AUTO_COMPACT_WINDOW=100000`, `DISABLE_AUTO_COMPACT=0`,
`DISABLE_COMPACT=0`, `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE=P`. Output-token settings
were untouched.

| Run | P | Expected formula trigger at R=20000 | Observed | Exit |
|---|---:|---:|---|---:|
| Fresh | 18.75 | 15000 | Successful two-turn tool interaction; no compact boundary | 0 |
| Exact resume | 18.75 | 15000 | Successful two-turn tool interaction; no compact boundary | 0 |
| Fresh | 9 | 7200 | Successful two-turn tool interaction; no compact boundary | 0 |
| Exact resume | 9 | 7200 | Two auto boundaries, 8039→1000 and 8570→1577 tokens; final result successful | 0 |

All init records selected `claude-haiku-4-5`. Model usage reported a 200000
context window and 32000 maximum output tokens. The last resumed turn continued
after compaction and completed successfully. The observed boundaries occur at
safe loop boundaries, not a hard token ceiling. These runs establish native
compaction during this isolated resumed interaction; they do not prove exact
trigger timing, arbitrary configuration, TUI execution, model fallback, or
output-budget stability. Fake-child tests cover both carrier argument paths.
No production model/version is promoted based on these observations.

## Source findings and remaining qualification

The pinned executable's embedded implementation has the formula at byte
206141266 (`SQ`) and reserve subtraction near 206145848 (`h5`). It uses
`min(effective max output tokens,20000)`, subtracts 13000 from usable context,
and takes the minimum with the valid percentage trigger. The explicit window
is bounded to 100000–1000000 and clamped to model context. Percentage-only
configuration can leave reactive mode active; an explicit window is required.

Settings environment values are applied after inherited child environment
(byte 202954444). Priority is user, project, local, flag settings, then managed
policy (197936020). Consequently both carriers merge compaction values into
`--settings` alongside their existing hooks. They do not change output-token
settings. Managed policy can override even those flags.

The output resolver at 207261478 and 204772930 reads configurable output limits.
Managed model restrictions at 200487578 can remap selected models. Remote policy
refresh, dynamic helpers and in-turn fallback cannot be bounded by the launch
version/model alone. Local managed-policy inspection can report observed
conflicting keys, but absence is not proof that a future server refresh has no
policy. Native remains off; fallback is explicitly best effort where managed
policy is present or cannot be inspected. A later promotion must prove actual
launch budgets and fallback models, not merely widen the version allowlist.
