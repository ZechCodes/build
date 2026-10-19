# Compaction wire and cache security checklist

Scope: #660, additive wire 3.22.0 and SPA adapter/cache support. The existing
harness qualification and session-policy checklists continue to govern actual
enforcement. Editors and the conversation menu belong to step 7.

Each control below is worth 10 points. **Score: 100/100.** All ten controls
pass. The final bridge suite passes 5,112 tests (14 ignored), and the full SPA
suite passes 10,818 tests across 701 files, including Chromium. Clippy, fmt,
API/manifest contracts, vendor/Pi checks, lint, build, captures, Semgrep,
gitleaks and diff checks pass; the #660 handoff carries the exit-code ledger.

| # | Control | Evidence |
| --- | --- | --- |
| 1 | Provider-map writes accept only the five declared harness keys and positive safe integers or provider null; malformed patches and mixed legacy/map writes are atomic | `compaction_wire_map_invalid_mutations_are_atomic`, `compaction_wire_scope_setters_reject_unknowns_zero_and_preserve_null_patch_semantics`, existing policy boundary and persistence-failure tests |
| 2 | Typed setters bind a server-owned project/workspace ID, reject unknown params, and cannot mutate an unavailable owner before answering | Scoped wire setter tests, parked-owner refusal assertion, strict param contract suite |
| 3 | Every configured, inherited and effective observation uses canonical ownership and provider resolution; legacy scalar compatibility remains distinct | `compaction_wire_scopes_and_unknown_active_are_authoritative`, existing all-provider/alias/project-worker/parked/adopted policy tests |
| 4 | Unsafe legacy values retain exact decimal read representations and cannot be submitted back as numeric mutations | Existing `compaction_policy_unsafe_legacy_values_read_exactly_without_rounding_or_clamping` extended through wire reads; `compactionContract.test.js`; adapter unsafe-integer test |
| 5 | A save cannot invent active policy, restart work or send a turn; unknown active and pending state stay null | `compaction_wire_saves_never_replace_the_applied_session_policy`, unknown-active scope test, SPA save-preserves-active test |
| 6 | The invalidation carries scope and owner only; delivery requires a current subscription, and queued/in-flight delivery is revoked with its kind | `compaction_wire_delivery_is_opted_in_and_revoked_with_the_subscription`, `compaction_wire_in_flight_event_is_fenced_by_kind_removal`, shared event fixtures |
| 7 | Subscription coalescing cannot lose a compaction invalidation, and the accepted subscription triggers authoritative catch-up | `compaction_wire_survives_a_prior_pending_overflow`, both requeue race regressions, and the unsent-thread cursor regression; SPA subscription acceptance, other-client invalidation and burst tests |
| 8 | Current greeting authority gates new params and setters; cached support is rendering evidence only and downgrade clears it | `deviceContexts.test.js`, `compactionCache.test.js` capability/old-bridge/downgrade tests; no new hello params |
| 9 | Cache writes are fenced against newer invalidations, records, generations and greetings, including inside delayed transactions; clearing an override clears stale cached values | `compactionCache.test.js` race/transaction/clear-override tests; shared settings-pull transaction guard and ordinary sync push fences |
| 10 | The change adds no credentials, config-file destinations, subprocess launches or live harness operations; verification is isolated and complexity allowances do not increase | Throwaway HOME/TMPDIR gate runs, full bridge/SPA/contract suites, existing complexity-ratchet tests, Semgrep, gitleaks and diff checks |

The service conservatively refreshes all held compaction dependencies on the
device because a partially cached ownership list cannot reliably identify all
inheriting conversations. It also reads the invalidated settings scope when
that record is cold. Reads are deduplicated and serial within a pass; a burst
shares one pass and one trailing pass, and a replacement session bypasses an
older stalled session. Only authoritative compaction fields are merged into
held agent records. Existing active work and unrelated cached roster fields
remain owned by their ordinary observations.

The feature capability describes the wire contract. It does not qualify a
harness version for native enforcement. Active evidence is labelled
`applied_at_start`; the bridge does not claim live upstream readback.
Legacy `compact_at_tokens` retains the conversation override or device scalar;
`compaction.configured` is the authoritative per-harness scope resolution.

`project.set_compaction` requires an available project record to return its
authoritative row. Parked project maps remain preserved by the existing policy
storage layer; an unavailable wire owner is refused without changing them.
