# Task #891 bounded task-reference integration

This slice admits point lookups from mounted task-page bodies and comments. It
is partial admission, not conversion of every reference or every Markdown
surface. Existing cached legacy task rows still supply positive links. There
is no catalogue drain, all-project point sweep, new search method, or request
issued by Markdown rendering.

## Sources and ownership

The production caller is `trackerTaskPage.js`: `take()` updates
`mountTaskReferenceLookups()` after adopting cached body/comment content. A
microtask inspects the resulting mounted Markdown. Index-triggered paints,
read/watch counters, title changes, and lookup results do not refill that
content turn's fixed selection. Explicit project task invalidations, current
greetings/reconnections, and settled local mutations may refresh that same
selection. Disposal and content changes revoke outstanding read authority.

`taskReferenceLookups.js` uses the existing `markdownRefs.referencesIn` grammar
for admission only; `markdownEntry.test.js` records that narrow ownership
exception. Rendering still goes through `markdownHtml`. The wire reader is
`taskAggregateReads.createTaskLookupReader`, sending only the negotiated #900
`tasks.lookup {project_id,number}` contract. Dispatch requires the live `lookup`
capability, the captured context identity, current greeting/session, and no
pending local mutation. Request installation additionally checks the captured
content turn and mutation epoch. Counts or summary-list support does not grant
lookup support.

`taskReferenceDemand.js` registers only explicitly admitted numbers.
`referenceIndexFeed.js` subscribes to their exact cache addresses; it performs
no point-cache discovery at bootstrap. Removed demand prunes both subscriptions
and transient source entries. IndexedDB retains the bounded point result for
future admitted use. `referenceIndex.js` and `referenceTargets.js` resolve
synchronously from those cached sources.

## Exact admission horizons

Each content turn admits at most four distinct positive safe-integer task
numbers. Its source capture examines the task body and at most the first 256
timeline rows. Only complete source parts of at most 4,096 JavaScript characters
are eligible; an oversized body or comment is left unadmitted in its entirety.
Eligible source text and exact comment-ID UTF-8 bytes together consume at most
16,384 bytes. Comment IDs must fit 128 UTF-8 bytes and contain well-formed
Unicode. Source descriptors and their fingerprint use the same eligible parts.
References beyond these horizons can remain unresolved; results do not trigger
successive admissions through the remaining content.

The mounted DOM walk visits at most 256 physical nodes, including element-only
nodes, to depth 32. It uses no whole-subtree selector or ancestor search. It
tracks the permitted body/comment scope while walking, skips code, preformatted
blocks and existing anchors, and examines text chunks of at most 4,096
characters within the same 16,384-byte text limit. A comment outside the source
row/byte horizon cannot become eligible merely because its DOM is nearby. Raw
class attributes over 256 characters leave that subtree unadmitted before any
class-token parsing. DOM comment IDs over 128 characters are cleared before
set membership checks, independently of the stricter source UTF-8 ID bound.

NUL in an HTML attribute can become U+FFFD, and carriage returns can normalize
to newlines. This reference-admission slice therefore leaves comment parts with
NUL, U+FFFD, CR or LF in their raw ID unadmitted, preserving neither an alias nor
a guessed association. Oversized or malformed IDs are also unadmitted. This
policy affects demand admission only; it does not rewrite stored IDs, change
other task controls, or redefine the canonical #900 marker domain. Tests cover
NUL versus literal U+FFFD turns and rejection of their superseded response.

## Unknown, found and missing

A legacy catalogue's absent row is always unknown here. Neither a held array,
a naked `catalog_complete` boolean, exhausted cursors, nor an unversioned union
of read stretches proves coherent query/basis/session completeness. Action
responses and `trackerTasksPane.cacheNewTask()` can manufacture one-row
catalogues, including records without page notes. They remain positive evidence
for their own rows and do not certify other task numbers missing. Only an
accepted point result with `status:"missing"` can supply a confirmed miss.

List rows and per-row read notes are read together through `readCachedMany`.
A row's observation clock uses its explicit per-ID note, or a span-derived
`taskReadOrder.lastSayIn` observation only when the catalogue's positive safe
`read_order` does not exceed that span's read. A later unnoted catalogue rewrite
cannot borrow an inherited old span's proof. Explicit per-ID notes remain
independent of a later unrelated row action's whole-list maximum. If an
explicit note predates a span whose lineage is now unproved, the row becomes
unknown rather than falling backward to that older note and resurrecting an
intervening stale point miss. Accepted overlay rows and rows without coherent
lineage have unknown clocks. Point publication stores local `published_result_read_order`; reserving
a later refresh preserves the previous result's clock, including when refresh
fails. These are local request clocks, not parsed or ordered server bases.

A proved newer point existence result may replace an older logical identity
or confirm its absence. Body/title freshness is separate: when both found rows
name the same exact task ID and have comparable valid `updated_at` timestamps,
the newer content is retained regardless of a later membership read that kept
an older body. The timestamp comparison follows the existing legacy page
folding convention; it supplies no absence proof. Different task IDs sharing a
number never borrow one another's timestamp authority. Source clocks and the
unknown-clock positive policy continue deciding that identity choice. Where
relative existence clocks are unknown, the known positive is retained. Refusal retains
cached point results and never schedules a retry by itself. Missing-result
freshness is tested through actual per-row action writes, an unrelated #99
action, and a failed refresh reservation.

The supported reference-number domain matches the existing point reader:
positive safe JavaScript integers. Node parses the adjacent JSON literals
`9007199254740992` and `9007199254740993` to the same Number. The former resolver
could link the second written reference to the first task and even relabel its
digits. The new guard leaves unsupported numbers as the exact written prose,
without a missing decoration or point dispatch. This records a visible old
consumer difference; it does not widen the RPC to strings or arbitrary-size
integers.

## Focused evidence and remaining scope

`referenceLookupIndex.test.js` covers partial/action-created catalogues,
source-pinned `cacheNewTask` shapes, no-note and mixed/interrupted spans,
transient demand churn, and atomic cache snapshots. `referenceLookupFreshness`
covers old misses/found titles, unknown clocks, retained publication clocks,
unrelated row actions and newer authoritative misses. Additional real-cache
fixtures preserve a T14 point title when `foldTasksPage` retains a T13 body
under a later read, reject inherited-span proof after a `cacheNewTask`-shaped
unnoted rewrite, and distinguish same-ID content freshness from different-ID
existence authority. `taskReferenceLookups`
uses the actual mounted task page to prove four links without result-driven
fanout, off-page unknown-to-confirmed-missing paint, and the unsafe-number
boundary. It also covers source/DOM/ID budgets, protected content, pending
mutations, old sessions, superseded content, unsupported greetings, refusal,
and disposal. Existing reference and Markdown ownership tests remain gates.

These tests do not establish full reference conversion, task-selector
conversion, or approval of the entire #891 consumer integration. Conversation,
plan, file, dashboard and other Markdown mounts still need their own explicit
bounded admission owner before they can dispatch point lookups. Provenance for
whole legacy catalogue absence is deliberately deferred. Potential full-read
producer sites are `cacheSync.readTasksNow`, `syncTaskPhases.wholeList`, and the
unnarrowed branch of `trackerTasksPane.readWholeList`; none receives a new
completeness marker in this slice.
