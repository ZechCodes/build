# Task chooser search: options for the epic lead

Decision input for #900/#891. This document proposes no capability, dependency,
schema, or implementation. Search remains a separate decision; none of the
choices below implies approval.

## Existing behavior and constraints

[`blockerChoices`](../spa/src/core/taskWaitingOnModel.js) trims the query,
calls JavaScript `toLocaleLowerCase()` without an explicit locale, removes one
leading `#`, and tests `includes` against the lowercase of the complete string
`${candidate.number} ${candidate.title}`. It excludes self and every existing
forward dependency, includes finished tasks, and preserves input order. The
query is not tokenized or normalized. Searching number and title separately
would lose substrings crossing their separating space. Neither case folding
nor Unicode normalization is an equivalent replacement for lowercasing.

[`MAX_TITLE_BYTES`](../bridge/src/tracker.rs) is 200 for supported new edits;
[`edits.rs`](../bridge/src/app/tracker/edits.rs) applies it to creation and title
edits. This does not prove that imported or historical records satisfy that
limit. The existing summary contract permits display-title truncation and
acknowledges large imported canonical records during migration; see
[`bounded-task-reads.md`](bounded-task-reads.md). Search must use canonical text,
including text past the display cap, or explicitly report unsupported data.
The present JavaScript helper imposes no query-length limit.

## Comparison

| Choice | What it preserves | Work and limitations |
| --- | --- | --- |
| Client supplies an explicit locale; server uses pinned Unicode/ICU rules | A reproducible, versioned server matching contract for supported locales | Does not guarantee identical results to every browser's current implicit-default casing. Requires defined locale fallback/rejection, versions, canonical search materialization and byte-bounded scanning. |
| Browser matches complete canonical candidates with its existing JavaScript operations | The calling runtime's actual existing casing and substring behavior | Bounded transport alone does not bound assembly, casing, matching, or total project traversal. Complete arbitrarily long titles require unbounded per-candidate client memory/work. |

### Explicit locale and pinned server rules

The client must capture its chosen locale and use it consistently for both
query and candidate transformations. Supplying a locale inferred from
`Intl.DateTimeFormat().resolvedOptions().locale` is a possible product policy,
not proof that it reproduces implicit casing in all runtimes. JavaScript offers
no casing `resolvedOptions()` API. Specify the policy rather than silently
guessing. A locale change restarts the query.

The server contract must identify algorithm, ICU/Unicode data versions, any
CLDR tailoring, supported locale behavior, and a stable matching-version token.
Reject unsupported locale/version combinations explicitly or document fallback
as changed semantics. Locale agreement alone cannot promise byte-for-byte
browser parity: ECMA-402 permits implementation-derived locale transformations
and tailoring, and browsers may ship different Unicode data. A finite probe
corpus can detect known differences; it cannot establish equality for all text.
See [ECMA-402 TransformCase](https://402.ecma-international.org/13.0/#sec-transformcase).

Preserve query preprocessing order and whitespace behavior explicitly. Sending
a browser-lowercased query to a server that lowercases candidates differently
does not fix parity. If adopting server semantics, both sides must use those
semantics or send the original query for server preprocessing. UTF-16 substring
semantics, valid canonical string encoding, expansion under casing, and
malformed/unpaired-surrogate query handling also need an explicit boundary.

For supported short titles, whole-string conversion has an enforceable maximum.
For arbitrary imported titles, precomputing a complete canonical search form
moves potentially large work to migration/writes; it does not make that work
constant. An on-demand conversion must instead expose resumable byte work and
account for contextual casing before claiming a request bound. An index over
full strings or a SQL substring call can still inspect unbounded bytes per row.
Locale-specific precomputation for every possible caller locale is not a
bounded storage or write strategy. Choose supported profiles or another design.

### Browser-owned matching over bounded content

The bridge could expose bounded canonical candidate content, separate from
display summaries, with exact identity, order key, content revision, offsets,
and continuation. This would need a separately approved content contract; it
does not exist merely because summary pages exist. The browser assembles the
complete `${number} ${title}` and calls the current operations in the same
runtime. That gives the strongest parity with this caller, including its
implementation-specific casing, provided query/default locale remain stable.
Results need not agree across browsers or browser upgrades.

Transport pages can respect candidate, decoded-byte and serialized-byte budgets.
Yet a title of B bytes requires at least O(B) transfer and complete-string
assembly/casing; a project with total title bytes T may require O(T) transfer
and matching to prove absence. Native JavaScript casing and `includes` have no
resumable-work API or specified complexity bound. Running them in a worker
keeps the UI responsive but does not establish bounded memory or CPU.

With a chosen maximum canonical candidate size L and query size Q, whole-string
matching becomes operationally capped per candidate, subject to measurement of
the chosen runtime. Oversized imports need an explicit refusal/fallback. Without
L, this option cannot honestly satisfy bounded per-candidate client work while
using the native whole-string operation. Query caps likewise change the old
unrestricted input domain and require an explicit error rather than truncation.
Client pending state must distinguish incomplete search from no match.

## Chunking is not a casing proof

Applying `toLocaleLowerCase` independently to chunks and joining the results is
incorrect. Unicode context rules include looking across runs of case-ignorable
characters for final sigma and across combining sequences for Lithuanian and
Turkic mappings. Those runs have no fixed maximum length. Any fixed overlap
can therefore miss relevant context. UTF-8 scalar-safe boundaries and grapheme
boundaries alone do not prove safe casing boundaries. See
[Unicode 16 section 3.13 and its casing-context table](https://www.unicode.org/versions/Unicode16.0.0/core-spec/chapter-3/#G33992).

A proved streaming implementation may retain context summaries, defer output,
or make additional passes; it needs its own correctness and resource argument,
including pending output and persisted state for arbitrarily long context.
Implementing it in the browser with pinned tables also gives up the promise of
using that runtime's actual native casing. Once a correct transformed stream
exists, substring matching can retain query-length state across chunks; this
does not solve the earlier casing-context problem. Matching overlap must be
defined in transformed-string units and account for expanding mappings.

## Continuation, order and completeness

Both choices need deterministic candidate ordering, exact exclusions, identity
deduplication, and a single match publication per candidate. A continuation must
bind store/project, query and preprocessing version, exclusions, locale/profile,
order, and traversal basis. If content is chunked, it must additionally bind
candidate revision and position; never concatenate chunks from different title
revisions. Changing selection, locale, runtime policy or basis invalidates the
search or starts a documented new traversal.

Candidates earlier in the order must be resolved before publishing a later
match as the first result. Sparse empty pages may carry continuation. A total
match count or definitive absence is unavailable until traversal completes.
No automatic cursor exhaustion during boot, input handling or background sync
may turn a request budget into an unbounded operation. User-driven continuation
can still eventually inspect the entire project; report that total cost honestly.
Restart-on-generation-change is simpler than a durable snapshot but can starve
under frequent writes. Snapshot retention and expiry would be additional scope.

## Recommendation and decisions still needed

Keep search decomposed from #900's other bounded queries. For exact preservation
of the calling browser's existing casing, prefer browser-owned whole-candidate
matching **only if** the lead accepts an explicit canonical-size/query-size
policy and user-driven progressive scanning. Do not describe arbitrary-length
chunk transport as bounded exact native-JavaScript matching. If reproducible
cross-client results are the priority, prefer an explicitly versioned server
contract and acknowledge that it changes the parity target from arbitrary
browser runtime behavior. Neither path is implemented or approved here.

The lead must choose:

- Browser-runtime parity versus pinned server semantics; locale ownership and
  behavior for unsupported profiles or differing runtime versions.
- Canonical title/query byte limits and treatment of oversized legacy/imported
  data, including explicit refusal versus separately approved fallback.
- Per-request candidate/byte budgets, client memory/work budgets, and whether
  user-driven full-project traversal is an acceptable total cost.
- Stable ordering, revision/basis invalidation, restart or snapshot semantics,
  and the UI meaning of incomplete search.
- If arbitrary-length exact streaming is required, separate algorithm design,
  proof, cost measurements and review before promising that guarantee.
