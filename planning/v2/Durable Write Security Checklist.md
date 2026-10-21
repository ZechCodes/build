# Durable write security checklist

**Status:** 100/100 (10/10 foundation controls verified).
**Scope:** #739, Step 6a foundation, wire 3.24.0 and Store schema 18.
**Reviewed:** 2026-10-21. Zech’s compatibility decision removes legacy opt-in
and rollback-build qualification, not process-crash integrity.

This score covers the foundation that ships in 6a. `ACK_CONFIRM_METHODS` is
empty and no production caller invokes `admit_mutation`; the new journal
therefore receives no user intents, bodies or secrets through the bridge.
The exposed operations are authenticated status/subscribe reads and process
epoch stamping. Store admission and recovery are exercised on disposable
stores by tests, ready for separately reviewed method adapters in 6b.

| # | Control | Score | Verification |
|---|---|---:|---|
| 1 | Status and subscribe take their principal from the authenticated bridge account, never a client/session hint or operation ID | 10/10 | `MutationService::read`, server-owned `DEVICE_USER`; `status_is_principal_scoped_and_current_authority_is_required`; authenticated carrier intake precedes the independent SQL lane |
| 2 | Possession of an operation ID grants no access. Reads and replay require matching principal and currently published original owner scope; unsupported or stale generations are denied | 10/10 | Principal-qualified journal queries; `MutationAuthority::permits`; cross-principal and removed-owner service tests; replay rechecks authority before delivery |
| 3 | No caller can enable a semantic write, select execution policy, persist arbitrary data or override authority through request metadata | 10/10 | Empty `ACK_CONFIRM_METHODS`; `WriteMetadata` rejects unknown fields; `write_metadata_is_refused_before_every_intake_fast_path`; metadata is refused with non-retryable `unavailable` in 3.24.0 |
| 4 | Operation/dependency sets and wire results are bounded before use | 10/10 | Canonical UUID validation, at most 8 dependencies and 128 status/subscribe IDs; `status_and_subscription_sets_are_bounded_and_cursors_are_decimal`; status/subscribe pages fit 512 KiB and continuation binds principal, journal and ordered ID set |
| 5 | Admission and terminal storage have byte/count limits, including non-ASCII UTF-8; a large outcome cannot leave committed effects unconfirmable | 10/10 | 64 KiB admission payload, 256 KiB complete terminal envelope; 128 active commands/8 MiB per principal and 4096/128 MiB store-wide; UTF-8 quota regression; precommit confirmation-envelope budget and malformed-terminal rollback tests |
| 6 | Durable identity and reservations cannot be reassigned by conflicting retries or another principal | 10/10 | `identity_is_durable_scoped_and_payload_conflicts`; normalized intent/hash tombstones; reservation inference/override and failed/published creator tests; one connection shared by legacy Store and StoreWriter |
| 7 | Process death preserves accepted identity and committed effects, without blindly replaying external work | 10/10 | `abrupt_worker_death_preserves_each_recovery_boundary_without_duplicate_effects` covers eight worker-exit stages; SQL-only work can resume, external claimed work becomes unknown, committed work waits for publication; forward migration and reconciliation tests |
| 8 | Subscribe closes the snapshot/registration gap, replaces the complete previous set and cannot emit from a stale opening, carrier or replaced set | 10/10 | Atomic cursor/state snapshot; snapshot-gap, released-carrier, reordered-completion and replaced-subscription regressions; `push_scoped` and identity gate; replay bypasses ChangeBus |
| 9 | The server stamps every authenticated reply, receipt and push with its own process epoch; producer/client values cannot select the fence | 10/10 | `carrier::epoch_tests`; stable-process/distinct-boot test; sender overwrites supplied epoch before authenticated sealing; persisted journal incarnation greeting and planned restart event tests. Epoch is a fence, not an authorization credential |
| 10 | Recovery reads avoid the app/writer guard, and retained history cannot cause an unbounded claim or idle outbox scan | 10/10 | 100 ms held-app/writer recovery-read test; active-owner index; 10,000-row principal/cursor outbox regression under 200 SQLite VM steps; one replay worker, bounded record batches, subscription replacement and closed-carrier cleanup; unchanged complexity ratchet |

**Total: 100/100.** Each control is worth 10 points. The named journal,
service and epoch tests passed in the clean 2026-10-21 Cargo run; the
complete run later failed in the unrelated project-worker deletion fixture.
Full readiness gates and their exit codes are recorded on #739.

## At-rest boundary

6a exposes no production journal admission path. It does not claim that its
SQLite database is encrypted or physically erases deleted bytes. The explicit
`compact_mutations` API removes completed intent/acceptance/result data after
30 days in batches of at most 256 records; no scheduler invokes it in 6a.
Partial/unknown outcomes retain recovery evidence until reconciliation, and
current tombstones can retain error text. Those facts are not an operational
retention guarantee for user data: method enablement remains gated below.

Tests use temporary stores and Cargo-built disposable workers. They do not
use an installed bridge, live daemon or the user's state directories.

## Required before any method is enabled (6b)

- [ ] Add automatic terminal compaction, including staged-data cleanup and an
      explicit policy for unresolved recovery evidence. Verify logical
      retention independently of any physical SQLite erasure claim.
- [ ] Decide which error text may remain in tombstones: redact or bound it,
      with tests. Define permitted intent fields so credentials are excluded
      and bodies follow the chosen retention policy.
- [ ] Publish `MutationRecovery` before readiness, with the enabled method's
      domain-specific recovery/publication adapter and tests.

These are unchecked 6b gates, not scored 6a controls. The distinction was
confirmed by the project coordinator on 2026-10-21 during #745 review.
