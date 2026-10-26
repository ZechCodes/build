# Claude compaction qualification — #656 / #884

## Current production contract — #884 (2026-10-26)

Native automatic compaction is enabled on Claude ADK and TUI. Build selects a
native window, not an exact trigger: requested 100k–1M maps directly to that
window; all other positive requests use `auto`. The child receives matching
`--autocompact`, settings and environment overrides. Output limits and native
precomputation defaults remain untouched. There is no competing automatic Build
scheduler while native remains available. Managed disabling policy is the
exception: `autoCompactEnabled=false` or truthy `DISABLE_AUTO_COMPACT` /
`DISABLE_COMPACT` selects `between_turn` at the unchanged requested threshold,
with the disabling reason on the wire. A window-only override stays Native.
The child still requests native enablement; managed policy takes precedence.
Manual `/compact` remains available unless managed policy disables it too.

The disable parser matches 2.1.284 (`1`, `true`, `yes`, `on`, trimmed and case
insensitive; byte 197482109). Later local fragments overwrite earlier scalar
and env values. Raw remote cache content does not prove active server policy;
those conflicts remain observable warnings, not fallback authority.
`DISABLE_COMPACT` also removes the manual command (byte 206239892). Below the
requested threshold wakes run normally. At the threshold, Build refuses the
wake and records one durable failed outcome/attention per agent conversation,
plus the original task/coordinator notice. Repeated wakes do not add failures,
send input or attempt an unavailable command. No wrapper replay is built.

The previous qualification rule required an exact requested-token trigger.
Zech explicitly accepted an inexact provider trigger for #884. Production now
reports approximately `min(window, model_window) - min(maxOutput,20000) - 13000`,
with an optional catalog-based range and no exact translated trigger. These are
estimates, not effective-budget or managed-policy readback. Native defaults
remain enabled even for unknown versions/models and unrepresentable windows.

| Requested window | Native setting | Approximate trigger with output reserve 20k |
| --- | --- | --- |
| 50k | auto | Provider-selected / reactive |
| 100k | 100k | 67k (model window at least 100k) |
| 150k | 150k | 117k (model window at least 150k) |
| 200k | 200k | 167k (model window at least 200k) |
| 1M | 1M | 967k on 1M models; 167k if clamped to a 200k model |
| Above 1M / unsafe legacy integer | auto | Provider-selected / reactive |

Lower effective output reserves raise these estimates, up to window minus 13k.
Managed model remapping, model fallback and policy can change them. New policy
applies on the next opening; an already-running child is not mutated in place.

The static CLI parser accepts `--autocompact auto` and integer windows
100000–1000000. CLI `auto` clears the runtime numeric window; the numeric
settings schema catches the corresponding string `auto` as unset, and an empty
window environment value is ignored. This delegates default resolution to the
provider (including clientdata/model defaults), rather than guaranteeing the
resolved source label is `auto`. Source offsets: bounds 197879173, settings
schema 198248395, parser 206142620, resolver 206144304, normalization 212330962,
CLI registration 214424667.

### Why native was disabled, and why #884 changes it

Commit `8f746defa21e27ad4fad9e7cc536c8e508050c92`
(`feat: normalize Claude compaction per launch with qualified translation`)
introduced the disable. Its source said: “launch version/model do not bound
effective output settings, managed policy refresh or native in-turn model
fallback.” Those uncertainty limits still apply; exactness no longer gates
native enablement. A fake child cannot qualify live summary success.

### Static reactive recovery evidence

Read-only inspection of the exact 2.1.284 executable below, matching its SHA256,
shows Build's old DISABLE_AUTO_COMPACT=1 disabled reactive recovery too. No
installed executable, live provider or bridge was run for #884; no configuration
or session stores were read. Zero-based byte offsets in its embedded source:

- 206140141: disable guard `vct` gates `Bf`, the auto-compaction predicate.
- 206144304 / 207000081: window resolution clamps explicit windows to model
  context; auto mode can defer threshold compaction to reactive handling.
- 206161686: `wme` recognizes a synthetic API error starting Prompt is too long.
- 206933172: `Nvr`/`SFn` require auto-compaction enabled, appropriate query
  source, no abort and remaining reactive allowance. Local mode enables the
  path; remote mode also depends on its reactive feature flag.
- 213031400–213034700: successful compaction replaces context and continues the
  original query. One normal attempt and one conditional deeper pass are
  bounded; exhaustion surfaces the provider error. Rapid-refill guards apply.
- 206927705: insufficient conversation groups, blocked hooks, aborts and summary
  refusals can defeat compaction. There is no promise every fixed prefix fits.

The fake-child reactive test models both a successful boundary/continuation and
terminal exhaustion, with one original wake and no wrapper replay/compact
command. The offline app integration test establishes that terminal refusal
reaches failure activity, task/coordinator notice and unread RunFailed attention.
On the first wake of an already-over-window resumed session after deployment,
Claude can attempt native recovery; if it emits a failed terminal result, Build
surfaces attention. Build does not replay across restart or create a fresh
handoff continuation; the task explicitly chose native recovery instead.

## Historical #656 qualification (2026-10-18)

The following records the original exact-trigger decision and isolated evidence;
its production-off statements are superseded by the current contract above.

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
| Fresh | 9 | 7200 | Auto attempt reported `compact_result:failed`; no boundary; turn still finished successfully | 0 |
| Exact resume | 9 | 7200 | Two auto boundaries, 8039→1000 and 8570→1577 tokens; final result successful | 0 |

All init records selected `claude-haiku-4-5`. Model usage reported a 200000
context window and 32000 maximum output tokens. The last resumed turn continued
after compaction and completed successfully. The fresh low-threshold run also
showed that a failed compaction attempt can leave the ongoing turn successful;
its failure status must not clear context or imply a completed summary. The
observed boundaries occur at
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
