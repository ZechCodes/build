# Serial gate failure, SQL credits and semantic test settlement

The coordinator serial gate `e4a105b5125b01a96030233a91f18e25bfb4cc02` failed.
The serial merge attempt used reviewed task head
`777322d0f8873cf84821ce214dfa500fab20111f` against unchanged dev base `12b8f869`;
it was not pushed. Preserve that gate's four original `.build/attachments`
reports and their actual outcomes rather than describing this failed gate as
passing after a later fix:

| Original gate component | Actual outcome |
| --- | --- |
| Cargo test | Exit **101**; **6,003 passed, 1 failed, 24 ignored**. The runtime RunFailed assertion expected 2 events and observed 1; a secondary PoisonError followed. |
| SPA test | Exit **1**; **11,973 passed, 1 failed**. SQL method parity omitted `tasks.counts` and `tasks.lookup` from sender credit admission. |
| Remaining reported gate components | Exit **0**, as recorded in the original attachments. They do not override either failing component. |

The prior `777322d0f` independent approvals and cost acceptance remain historical
approvals/evidence for that head. Descendant code changes require fresh gates
and exact-head reviews; they do not inherit a new approval merely because an
ancestor was reviewed. The controlled experiments below establish the same admissible observation
ordering on both base and candidate. They do not reconstruct the unique
schedule or frequency of the original user-gate failure.

### Cross-tier SQL-read validation map

The SPA omission is a gate integration finding: new bridge read verbs must be
paced by the existing opening-scoped sender admission system even while new
bounded consumers remain held. This repeats an integration category already
encountered in #890; holding new view activation does not exempt the shared
request dispatcher from method parity and physical credit coverage.

Commit `228d921e3959f6a52a29524ff07e228abb028949` adds counts/lookup to the SPA
SQL method list and exercises the actual dispatcher. This is not only a list
or mock-key comparison:

| Producer / dispatcher boundary | Executable validation |
| --- | --- |
| `bridge/src/api/effects.rs::SQL_READ_METHODS` → `spa/src/core/sqlReadCredits.js::SQL_READ_METHODS` | `sessionSqlReads.test.js`: “paces bridge SQL routing and the 3.24 durable reconciliation reads” compares the bridge method set plus durable write reads with the sender list. |
| Counts/lookup share eight credits with mixed SQL reads | “shares counts and lookup credits with mixed SQL reads at %s priority”, for both interactive and background priorities, starts eight mixed calls, queues additional counts/lookup, and observes the real sender's queued/admitted snapshot. |
| Error and success release physical admission | The same cases release counts on a structured unavailable error, lookup on success, admit the next queued method, and check the remaining six admitted calls rather than silently abandoning them. Control `ping` remains unpaced. |
| Opening ownership spans priorities and carriers | “paces every SQL method across priorities and carriers while control and other reads pass” checks shared admission across carrier changes; existing dispatcher suites also retain reply/receipt/timeout release fences. |

Actual focused SPA evidence used Node **22.23.4** and npm **12.0.2**:

- `/tmp/task900-spa-baseline777-red.log`: baseline parity test **1 failed,
  24 passed**, exit **1**, witnessing the omission before the repair.
- `/tmp/task900-spa-regression-red.log`: expanded dispatcher regressions
  **4 failed, 23 passed**, exit **1**. Counts/lookup escaped the eight-credit
  budget before the method-list repair.
- `/tmp/task900-spa-focused-green.log`: **85 passed**, **3 test files passed**,
  exit **0**, covering `sessionSqlReads.test.js`, `sessionRpc.test.js`, and
  `sessionRpcTiming.test.js` after the repair.
- `/tmp/task900-spa-eslint.log`: focused ESLint exit **0**.
- `/tmp/task900-spa-semgrep.log`: focused Semgrep exit **0**, **0 findings**
  on the two tracked changed JS files.

These focused passes establish the repaired SQL-credit integration for the
named source, not a pass of the original failed user gate or a complete SPA /
bridge rerun on the eventual final head. The audit-writing subagent read the supplied logs and sources and performed no builds, tests or scans for this addendum.

### Controlled runtime diagnosis

The runtime owner ran the same test-only diagnostic overlay on unchanged base
`12b8f869bd94e259666df69f1a8c0cde5c695982` and candidate
`228d921e3959f6a52a29524ff07e228abb028949`, serially. The actual ADK child,
BusyStore fixture, initial/refusal/repeated guard, 100 ms writer and eight
readers remained in the experiment. The gate matched the exact operation and
AppState Arc, and paused only the actual background `spawn_blocking` runner
before receipt/ticket settlement, with no AppState mutex held. It did not pause
the synchronous helper or alter production delivery semantics.

The protocol is `/tmp/task900-managed-settlement-protocol.txt`; the identical
overlay `/tmp/task900-managed-settlement-overlay.patch` has SHA-256
`3cad81d3eb4522463ee7cb521de4b1a6cff6116c4dc2f36a7d8483e9cc86274e`.
Immutable test binaries were copied/hashed before the next compilation:

| Controlled source | Binary SHA-256 | Actual result |
| --- | --- | --- |
| Base `12b8f869` | `e9a62c609b9ab44e68f67dc9beae4b28a004bde370c04be15c1372c5bf75eae0` | Exit **101**, 0 passed/1 failed; `/tmp/task900-managed-base-controlled-red.log` |
| Candidate `228d921e3` | `3abf4aeeed37f34bf25afcfc4f066ef6204aff84fe4781f7e25aabeb2b9fa73e` | Exit **101**, 0 passed/1 failed; `/tmp/task900-managed-candidate-controlled-red.log` |

Both logs observe the same causal sequence:

| Observation | Held before background settlement | After gate release |
| --- | --- | --- |
| Queued turns | 1 | 0 |
| Agent in flight | true | false |
| Below-threshold receipt | Claimed | Settles before the queued cycle completes |
| New blocked-cycle receipt | Queued | Delivered with the exact managed `DISABLE_COMPACT` refusal |
| Recovered failure marker while held | false | New refused cycle records its result |
| Visible failed messages | 1 | 2 |
| Real child input lines | 1 | 1 |

```mermaid
sequenceDiagram
    participant T as Test
    participant P as Model and pump
    participant B as Background delivery
    participant Q as Queued next cycle
    B->>P: Send below-threshold turn
    Note over B: Gate holds before receipt and ticket settlement
    P-->>T: Existing model/pump predicate succeeds
    T->>Q: Queue next managed-blocked cycle
    Q-->>T: Still queued; failed-message count is 1
    T->>B: Release settlement gate
    B->>Q: Ticket clears; queued cycle can run
    Q-->>T: Delivered refusal; count is 2; child input remains 1
```

The old helper observed model/pump completion before the background delivery
receipt and in-flight ticket necessarily settled. The next cycle could still
be queued when its strict two-failure assertion ran. Both controlled tests
recorded that old immediate value, released/cleaned up the gate, verified final
settlement and **only then** evaluated the original expected-2/observed-1
assertion. They failed that assertion without a PoisonError. This separates the
primary observation race from mutex poisoning caused by assertion unwinding in
the original test.

The base overlay was reverted using the saved patch; its HEAD remained
`12b8f869` and its checkout was restored clean. Independent read-only source
diagnosis by `/root/final_code_review` inspected the completion/settlement gap;
that reviewer did not execute these experiments. The experiment owner `/root/projection_impl` ran them; the audit-writing
subagent read the protocol/logs and source changes without running builds or
tests. The lead accepted the controlled proof and its limits.

The result proves the ordering was possible on both the old base and candidate.
It does **not** uniquely identify the original `e4a105b` schedule, establish
incidence/frequency equality, or attribute any earlier 209 ms `agent.deliver` frame to
this test race. No such timing or no-regression inference follows.

### Test synchronization repair and verification ownership

The test-owned synchronization repair is committed as
`3a04e3926072b0b756f43c43923e2ffe70bb928a`. `ManagedBlockFixture::wait_for_delivery_settlement`
waits for the **exact operation** to be Delivered **and** the agent to have no
in-flight delivery. Only after this semantic completion does the test check the
expected error and strict exactly-once failure count. It does not replace the
assertion with a sleep, retry the failed assertion, or relax the expected count.
The below-threshold operation must settle before the next cycle is asserted.

`managed_block_semantic_completion_waits_for_background_ticket_settlement` uses
the same controlled real-runner barrier to verify the completion future remains
**Pending** while the ticket is held and becomes **Ready** after release. It
then checks the exact managed refusal, two failures, two coordinator notices,
one child input, and another due wake remaining at two failures. The original
BusyStore/100 ms writer/eight-reader assertions and existing timeout bounds are
retained. Strict failure/task-comment/child-input assertions run both before
and after the original two-coordinator-notice wait window, preserving the
post-window delivery guard rather than moving it earlier.

The seam module and call sites in
`bridge/src/app/runtime/delivery/runner.rs` are all behind `#[cfg(test)]`;
`settlement_probe.rs` belongs only to that test build. Inspection therefore
finds no non-test Rust runtime behavior change from this synchronization repair.
This is a source-scope conclusion, not a completed final gate or review.

The first planned focused compile at
`/tmp/task900-managed-semantic-sync-green.log` was **cancelled with exit 130
before test execution** after independent source review caught a moved
post-coordinator-window input assertion. It receives no test-pass credit despite
its filename. The source restored strict failure/comment/input assertions on
both sides of that original wait window.

The fresh whole `adk_turn_failures` focused run exited **0**, **7 passed**,
in 60.90 seconds (`/tmp/task900-managed-semantic-sync-final-green.log`). Its
emitted unit-test binary SHA-256 is
`d5eaa40e8f1f22b7dc0deb836ac5203898fe6f904ab4ad82268cdc3af5152237`.
The working source was subsequently committed as `3a04e3926`; this focused
result is not a substitute for full Cargo on the frozen final candidate.

Exact focused command from the candidate `Build/` root, with no command-scoped
environment overrides:

```sh
nice -n 10 cargo test --manifest-path bridge/Cargo.toml --lib app::tests::adk_turn_failures -- --nocapture > /tmp/task900-managed-semantic-sync-final-green.log 2>&1
```

The cancelled compile used the same command, differing only in log path. The
source-scoped conclusion that Rust production is unchanged is not a new binary
latency measurement.

The final frozen candidate's full command/exits and independent dispositions
are recorded on #900. This source document records the diagnosis and acceptance
checks, rather than crediting a later source revision to an earlier execution.

| Acceptance check | Evidence and attribution |
| --- | --- |
| Original gate failure | Four retained attachments and exact failed merge head; the original merge was not pushed. |
| Same-overlay diagnosis | Both exact-source controlled RED runs exited 101 with the provenance/limits above; baseline restored clean. |
| Independent diagnosis | `/root/final_code_review`, read-only inspection of seam, logs and semantic synchronization; no test-execution claim. |
| Focused synchronization group | First compile cancelled 130 before tests; corrected seven-case run exited 0 for source committed as `3a04e3926`. |
| SPA credit integration | Source `228d921e3`; 85 affected cases pass with real eight-credit behavior and parity coverage. No full SPA suite is requested. |
| Final full Cargo and lint/format/scans | Required on the frozen final candidate; exact source, commands, raw logs and exits are posted on #900. Focused results are not substitutes. |
| Independent final scope/code/Astra reviews | Each disposition must name the final exact head and inspect the synchronization and client-admission delta. Ancestor approvals do not self-approve the descendant. |
| Re-handoff publication | Requires completed gates, reviewed exact descendant, a non-rewriting task-branch push and remote verification. The coordinator controls serial integration. |

No new timing run is inferred from this documentation or the SQL-credit list
repair. Existing accepted cost evidence remains tied to its measured source;
the lead requires no new read-latency run for this cfg(test)-only repair if
that production-scope boundary is confirmed. A later production behavior change
would require reassessing appropriate evidence. Existing #891 marker, pending-Any and rendered mouse/Enter activation gates
remain separate and held. Exact backend semantics are unchanged by this
validation addendum.
