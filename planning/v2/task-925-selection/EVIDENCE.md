# #925 research evidence and disposition

**Result: bounded-turn exact builds are a viable research direction, but the
proposed availability envelope fails for some quiet workloads. Production
implementation/activation/cost acceptance is not authorized or established.**
A project epoch removes unrelated-project invalidation; it cannot guarantee a
stable selected-project result. Smaller admission, more duty cycle, a longer
budget, refusal support, or retained versions would require explicit decisions.

## Provenance and validity

Base is `fcec698761695148bcd8736e9bb516aefa00a8e0`. The measured Python
prototype/runner/tests are commit `bdb296735c35dd45aa88a69908e43eb4e8248f4a`.
`evidence/provenance.json` pins their actual measured SHA256s, Python/SQLite
versions and current comment-only scan annotations. AST equality was checked:
there is no behavioral difference between measured code and those annotations.
`source_cost.py` and `domain_experiment.py` have their measured hashes in
`evidence/source-sha256.txt`. The entire final 32-scenario matrix was rerun
against the frozen post-fix code. `all-final-scenarios.jsonl` contains exactly
4 sizes x 2 bases x 4 mutation schedules; uniqueness/count assertions pass.

The earlier `initial-counts.jsonl` is retained solely as a superseded diagnostic.
It used the corrected stride-one VM/normalized-key worker but predates the final
sealed-input refcount/GC changes. Its full in-flight source snapshot was not
captured, so it cannot be pinned to an exact complete code revision and **no
conclusion below relies on it**. Earlier coarse-callback and expression-index
results are invalid for boundedness; their failing tests are retained. All
reported measurements below come from final post-fix runs, not extrapolated old
counter values. The sign-in interruption left the original runner alive; it
finished exit 0 with 32 rows. Process inspection confirmed no surviving runner
before final runs started. No process was stopped or duplicate gate restarted.

All measurements use isolated synthetic in-memory SQLite fixtures, under an
ambient host with concurrent work. There was no reserved timing window.
Wall-time fields are exploratory and are **not** latency gates, performance
comparisons or evidence for the bridge's <=100 ms requirement. Python stride-one
callback overhead differs from Rust. Source fixture writes run between worker
transactions on one connection; this is **not** concurrent SQLite lock/writer
contention, WAL/disk behavior, Store migration, carrier admission or app-lock
measurement. All source writes stay outside installed/home/live bridge state.

## Deterministic scan and service envelope

Every scenario stages 1,001 known actors and 1,001 configured statuses. Setup
counts create/append/seal calls, with chunks <=100. Work checks project/global
epoch, scans <=64 headers and <=64 relation facts independently, and publishes
only in a later transaction with matching epoch. The final code enforces a
100,000 VM-instruction interruption threshold and a 1,024 indexed-probe cap.
Successful VM counts use stride-one callbacks while the guard is installed;
`vm_upper` is the historical field name for that exact callback count. It does
not count VM work after the handler is cleared for rollback. SQLite internals,
CPU instructions, preparation, recovery and lock waiting are not bounded by
this number. Rollback work has a bounded changed-row footprint but requires
separate measurement/accounting before production.

The **modeled** service policy is 10 quanta/second globally, with 1,200 total
charged setup/admission/worker turns (120 seconds), initial attempt plus at most
two restarts. No wall-clock sleep simulates this rate; the service seconds below
are charged turns divided by ten. Queueing and actual scheduling can only add
latency. Cleanup runs **after** that envelope and is reported separately: it
must retain charged storage/reserved maintenance capacity, rather than quietly
being called free or included in the 120-second completion budget.

| Quiet project-basis workload | Setup + admit + build turns | Build turns | Model earliest result | Publication |
| --- | ---: | ---: | ---: | --- |
| 25 tasks | 31 | 2 | 3.1 s | exact 25 |
| 1,000 tasks | 46 | 17 | 4.6 s | exact 1,000 |
| 10,000 tasks | 187 | 158 | 18.7 s | exact 10,000 |
| 100,000 tasks | 1,200 exhausted | 1,171 spent | no result in 120 s | unavailable, no total |
| 10,000 tasks, 3 overlapping labels each | 486 | 456 | 48.6 s | exact 10,000, no overlap double-count |
| 1 task with 503 relation labels, match at last fact | 39 | 9 | 3.9 s | exact 1 after full positive evaluation |
| Three 10,000-task builds, shared sealed inputs, fair rounds | 505 globally | 474 globally | last by 50.5 s | all 3 exact |
| Sixteen 10,000-task builds, same shared inputs | 1,200 exhausted | 1,156 globally | none in 120 s | all 16 unavailable |

100,000 tasks need at least 1,564 build turns plus 29 input/admission turns in
this fixture: 159.3 modeled seconds for one attempt even without mutation.
Sixteen 10k builds need 2,572 charged turns if all run to completion, exceeding
the envelope. **The initial 16-job/120-second proposal fails even in a quiet
store.** It is not an approved quota. Three jobs sharing inputs are a measured
passing cohort, not proof that every three-job selection fits; large relation
sets, independent input staging and cleanup debt change the envelope. Admission
must account for reserved cleanup and finite deadlines, and a future smaller
active-job policy must be measured, not inferred to cover arbitrary N/L/R.

Across the final size/schedule matrix, maximum successful worker work is 64
headers, 14,710 encoded header bytes, 194 indexed probes, 325 SQL executions,
and 9,899 counted VM instructions. Overlap bounds the relation channel at 64
facts/quantum and observes 29,092 fetched facts total; OR may short-circuit
before visiting every existing label. The 503-label case observes all 503 facts,
<=704 fact-value bytes/quantum, <=68 indexed probes and <=1,198 VM instructions
per turn. Header bytes and label-value bytes are measured logical payload
subsets; they omit set-probe values, result/index bytes, SQLite pages and WAL.
Thus they **do not validate** DESIGN's proposed complete read/write byte ceilings.
The prototype enforces per-input/per-header domains, not full physical byte IO.

Setup max is 2,953 counted VM instructions/call. In these cohorts GC removes at
most 64 records/call; max successful cleanup VM is 2,148. Cancellation, input
release and seal are guarded metadata operations, and shared sets remain
referenced until all dependent build rows are collected. Expiry scanning,
complete request ownership, input chunk receipt replay, physical reserve and
production quota enforcement are still design requirements, not prototype
features. Cleanup after quiet 10k is 209 additional calls; after the 100k
unavailable attempt it is 1,224 calls. Deletion debt can outlast the attempt.

## Mutation and finite retry results

Schedules are deterministic stress fixtures, not empirical workload incidence:
`normal` injects one same-project mutation before worker turn 5, then becomes
quiet; `sustained` mutates the selected project every four global worker turns;
`unrelated` writes another project at that cadence. Mutations change normalized
source facts and epochs atomically. A tiny build can seal before the first
scheduled mutation; that is a completed interval, not immunity to mutation.

For 1k/10k tasks, under either basis normal mutation invalidates the first
attempt and the second completes (10k: 193 charged turns). Sustained
same-project mutation invalidates all three attempts under both bases; 10k
stops after 44 charged turns with no exact products. Under unrelated writes,
the global basis likewise loses all three attempts (44 turns), whereas project
basis completes 10k in one attempt (187 turns). Quiet global and project runs
both complete 10k, proving no assumed snapshot/journal is needed in a quiet
interval. At 100k, even quiet/project or unrelated/project exhausts the finite
work envelope without completion; a narrower basis alone does not fix scale.

No intermediate count, ordered traversal or complete status discovery is
readable while pending, invalid or unsupported. The prototype refuses reads
when a sealed result's epoch changes. DESIGN additionally specifies cache
payload version retention, local-mutation fences and separately refreshed
identity decorations; those client rules are not implemented here.

## Storage and producer costs

SQLite page allocation and dbstat count in-memory database/index pages. They are
physical-format observations, **not** disk usage, WAL maximum, RSS or freed-space
reclamation measurements. Source/input baseline is excluded from the delta;
input facts still incur real storage and are shown separately in raw JSON.

| Exact result fixture | Added allocated SQLite bytes before cleanup | Logical membership payload bytes |
| --- | ---: | ---: |
| 1k selected | 462,848 | see raw result table |
| 10k selected, 1,001 statuses | 3,887,104 | 782,858 |
| Three shared-input 10k builds | 11,587,584 | 3 x 782,858 |
| 100k attempt stopped at envelope | 29,532,160 | partial private memberships only |
| Sixteen 10k attempts stopped at envelope | 29,372,416 | partial private memberships only |

At 10k, 1,001 status records add 53,055 logical payload bytes; staged 2,002
roster/config entries add 90,092 logical payload bytes before B-tree/index
costs. The total baseline allocation is 3,379,200 bytes and completed allocation
7,266,304 bytes. Several membership indexes make allocation much larger than
raw record payload. Logical deletion does not immediately shrink allocation.
64 MiB/build, 16 MiB/set and 256 MiB/global in DESIGN are **unvalidated candidate
quotas**, not accepted capacity limits or guarantees for all valid selections.
The observed failures mean throughput/deadline policy needs a separate decision.

`source_cost.py` measures the **research exact-fact fixture producer**, with
exact stride-one counts outside build budgets. It is not a matched baseline
against Build's existing Rust producers, nor permission to change them:

| Labels replaced on one task | First insertion VM steps | Same-value replacement VM steps | SQL executions per write |
| --- | ---: | ---: | ---: |
| 0 | 104 | 124 | 7 |
| 3 | 164 | 204 | 10 |
| 503 | 10,164 | 13,704 | 510 |
| 10,000 | 200,104 | 270,123 | 10,007 |

The fixture deliberately uses whole-task relation DELETE/INSERT, outside build
quanta. The 10k-label producer exceeds the worker VM ceiling by itself. This
makes O(L) producer cost concrete; it is **not** a bounded production producer
proposal. Canonical writes cannot start failing due to a selection budget.
A lossless exact-fact migration/producer design, indexed unsupported flags,
per-project epoch hook cost, actual storage/WAL and matched Rust producer
regressions remain unmeasured and require separate scope/cost disposition.
No prior #890/#900 acceptance is reused.

## Domain/refusal and consumer correctness coverage

22 tests pass on final post-fix code. They cover sealed distinct counts and
immutability, >100 roster/choice entries, >500 statuses, overlapping OR labels,
503-label resumption, three disjoint groups and ordering, actual-agent W with
explicit empty versus absent, caller-resolved coordinator presence/missing/legacy
input, open cancelled tasks remaining unfinished, configured Backlog readiness
versus blocked, non-Backlog number order, configured columns including zeros,
Board ignoring user status filter, stray Backlog remaining ungrouped,
pending-not-zero, mutation before seal/after publication, global/project
invalidation, durable cursor reopen, input/VM rollback, cancellation and bounded
shared-input GC. An off-page `supported=false` candidate after multiple worker
pages makes a would-be empty selection refuse; exact totals remain unreadable.
The flag models source validation, **not actual Rust canonical-projection parity**.

`domain_experiment.py` preserves eight synthetic canonical invalid candidates:
excess label cardinality, >40B multibyte label, distinct >128B status aliases,
>128B agent becoming NULL, >128B multibyte status, and empty/untrimmed labels
retained with invalid flags. Supported NUL and 128B multibyte status bytes survive.
Ten illustrative open-state selections all refuse under conservative project
invalidity, while a full scan can prove the closed invalid tasks outside F and
refuse none (10/10 versus 0/10). Closed-state selections refuse 10/10 under both.
New exact relations recover the two cardinality/long-label cases but opaque-ID/
status and malformed-label restrictions still refuse these closed queries.
This quantifies a constructed refusal impact, **not its prevalence in user data**.
No SQL from the real Store producer runs in this domain model. Full production
parity fixtures must cover creator/reference IDs, marker fields, number overflow,
all producer paths and legacy/canonical writes and off-page source data.

Prototype consumer gaps are deliberate and block any production parity claim:
status metadata pages are lexical, so ordered filtered stray discovery is only
a design contract; the prototype explicitly refuses empty status values and
duplicate configured IDs, while DESIGN requires preserving both legacy cases;
there is no full state/assignee-selector implementation or pending_present `any`
filter; attention is a precomputed boolean and coordinator a resolved scalar.
No wire ownership/capabilities, SPA integration, activation/backfill/expiry,
carrier output-byte envelopes or live identity-decoration fencing are implemented.

## Corrections, gates and remaining reviews

Independent Sol scope/source review requested and obtained corrections for
`any` versus held, full non-decoration card epoch coverage and invalid-label
source details; it then approved the design/research scope. Separate Sol
architecture review identified actual VM and index traps and semantic oracle
errors; fixes were applied before all final measurements:

* The initial `callbacks*100+99` VM estimate undercounted residuals across
  prepared statements. Final stride-one regression executes 1,000 distinct
  short statements and counts >=5,000 instructions.
* Expression-index tuple seeks could scan earlier same-rank rows. The old
  10k-row late-page test hit the 100k VM interruption budget (red). Stored
  normalized key columns produce full tuple seeks. Final test checks EXPLAIN
  contains the number key and late-page VM stays within 2x first-page +500.
* Workspace membership, unfinished semantics, readiness scope and normal Board
  order were corrected in both prototype and independent Python oracle.

Red/green command and exit evidence is retained in `evidence/tests-*.txt`.
Final unittest command exits 0 (22 tests). `domain_experiment.py` and
`source_cost.py` exit 0. Each of the eight final runner invocations exits 0,
including unsupported/progress-failure scenarios represented as explicit results.
There were no hidden failed process exits treated as success.

Semgrep's first explicit-path scan exits 1 for six formatted-SQL findings.
Manual inspection shows each interpolation is a fixed internal diagnostic
identifier/expression, fixed order/placeholder/table variant, or trusted range
integer in the distinct-statement VM test; all external data values are bound.
Narrow rule-specific annotations carry those reasons. The subsequent scan of
all five Python files exits 0 (290 rules, five files, no remaining findings).
Annotations are AST-identical to the measured code. Original scan output is
retained. Final diff/security scans and independent architecture recheck are
recorded in REVIEW.md/commands.txt when complete.

No full SPA, Rust build, installed bridge, browser, deployment, journal/detail/
transport change or production timing was run. No isolated latency/throughput
claim or blanket cost acceptance follows from these tests. If an isolated
research timing follow-up is useful, the concrete command is `nice -n 10 python
planning/v2/task-925-selection/measure.py --rows 25 1000 10000 --basis project
--schedule quiet --jobs 1 --max-quanta 1200 --soft-ms 20` (reserve approximately
one minute, record host load); this still cannot replace Rust carrier/writer
release measurements. No such window is reserved or run in this packet.
