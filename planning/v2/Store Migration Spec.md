# Bridge Store — SQLite Migration Spec

**Status:** Shipped — phases 1, 2 and 3 are on `main` (merged as `397f770`)
**Last updated:** August 21, 2026
**Relates to:** `Agent Session Interface Spec.md` (§6.2 is superseded by this), `Harness Refactor.md`

---

## 1. Why

Bridge state is JSON files, one aggregate `record.json` per Issue holding that
Issue, every implementation inside it, and every thread on all of them. Saving
anything is a read-modify-write of the whole aggregate: read it, pretty-print
all of it, fsync (`bridge/src/store.rs:615`, `:1242`).

Measured on a real store, 13 issues:

| record.json | thread items | implementations |
|---|---|---|
| 593 KB | 134 | 1 |
| 538 KB | 85 | 5 |
| 322 KB | 32 | 1 |

**593 KB rewritten and fsynced per state transition**, holding only messages and
lifecycle events. Two consequences, and the second is the one that actually
blocks product work:

1. **Write amplification.** Every append rewrites a file every previous append
   made bigger. Adding agent activity (reasoning, tool calls) at hundreds of
   events per session makes this quadratic — roughly 200 MB written per session
   at today's record sizes.
2. **No pagination.** A conversation is a `Vec` that is fully resident in memory
   and shipped whole on first load. A sitting that produces dozens to hundreds
   of entries has no way to send the client a bounded slice.

The forward cursor already exists — detail polls send `thread_after_sequence`
and get only what is newer (`thread.rs:1957`). What does not exist is a
**bounded first load** or any way to page backwards.

### The rule this corrects

"Everything is files + git + PTY" is about what the **user** sees and what the
**agent** operates on: files in a tab, git in a tab, a terminal when you want
one. It was never a claim about how the daemon stores its own bookkeeping. Using
it to justify JSON-file persistence was a misreading.

---

## 2. What moves, and what does not

### Into SQLite

| Today | Becomes |
|---|---|
| `issues/<id>/record.json` — issue + lifecycle | `issues` row |
| …the implementations inside it | `implementations` rows |
| …the threads inside those | **`thread_items` rows** — the load-bearing change |
| …sessions, revisions | `thread_sessions`, `thread_revisions` rows |
| `runs/<id>.json` | `implementations` rows (planless) |
| `captures/<id>.json` | `captures` row |
| `attention/map.json` — whole map rewritten per change | `attention` rows |
| `archived-worktrees/` | `archived_worktrees` rows |

### Staying on disk

| Stays | Why |
|---|---|
| `issues/<id>/docs/**` — stage plan docs | Markdown the **agent** reads and writes in a worktree. Materialized into checkouts and ingested back. A blob in a database that has to be written to disk to be useful belongs on disk. |
| `attachments/` | Reviewer-supplied files, handed to agents by path. Same reason. |
| `identity.json` | A keypair at `0600`. Unrelated concern, no benefit to moving. |
| `config.json` | Hand-editable by design. |
| Git worktrees | Obviously. |

**Only `thread_items` needs normalizing.** Everything else is small and bounded,
so those rows can carry their existing serde shape as a JSON column and be
migrated properly later if a query ever needs it. Normalizing them now buys
nothing and multiplies the diff.

---

## 3. Why SQLite alone does not fix pagination

Worth being explicit, because it decides the phase order.

`AppState` holds `plans: HashMap<String, ActivePlan>` and `runs: HashMap<String,
ActiveRun>` (`app.rs:1716`), each carrying an `AgentRoster` with full `Thread`s.
Boot loads every record into those maps and **every read is served from memory**
— the process is the index.

Swapping the persistence layer under that changes what a *write* costs. It does
not change the fact that the entire conversation history of every issue is
resident, nor that `wire_value_after(0)` ships all of it. Pagination needs the
thread to stop being a fully-resident `Vec`, which is a separate change to a
different part of the code.

So: **phase 1 fixes writes; phase 2 fixes residency; phase 3 fixes the wire.**
Phase 1 alone is worth shipping, but it is not what makes conversations
paginate.

---

## 4. Phases

### Phase 1 — SQLite behind the existing `Store` API

The `Store` is already a repository boundary: ~25 public methods, and `app.rs`
never touches a state file directly (48 call sites, all through `Store`). The
API shape survives; the backend changes under it.

- Add `rusqlite` with the `bundled` feature — SQLite compiles from source and
  links statically, so there is no system dependency on a user's laptop and
  nothing changes for scratch containers.
- Schema, with `thread_items` normalized to rows. Append becomes one `INSERT`.
- `save_issue_plan` / `save_issue_implementation` / `save_run` / `save_capture` /
  `save_attention` reimplemented as row writes inside a transaction.
- Boot migration reading the JSON records and writing the DB. **Idempotent**,
  and the JSON tree is renamed rather than deleted — the existing store already
  has a `tasks-backup-…` directory from a previous migration, so this follows a
  precedent.

**Unlocks:** 593 KB per transition → one row write. Crash safety is unchanged
(WAL replaces tmp+rename). No behaviour change, no wire change.

**Size:** the bulk of the work. `store.rs` is 3411 lines and 71 tests; most of
those tests describe record shapes and boot migrations and can be pointed at the
new backend rather than rewritten.

> **Phase 2 shipped 2026-08-22.** `RESIDENT_CONVERSATION_TAIL` is 200: a boot
> reads the newest items of each conversation and remembers how many it left
> behind. The audit below was done and the split held — but the sharp edge this
> spec warned about did bite: unread badges were counted off the tail and
> under-reported. The fix was to make the class a column (`thread_items.attention`,
> schema 2) so the badge is a `COUNT` between the read cursor and the resident
> floor rather than a scan. See `ca54ebd`.

### Phase 2 — the thread stops being fully resident

`Thread::items` becomes a bounded window over the table rather than the whole
history.

The audit this needs is the real content of the phase: **45 sites in `thread.rs`
and 39 in `app.rs` touch `.items`**. Each has to be sorted into one of two
groups:

- **Needs the whole history** — unread counting, attention reasons, the last
  event for a digest. These become queries (`SELECT COUNT(*) WHERE …`), which is
  what a database is for and is *cheaper* than today's scan.
- **Needs only the window** — rendering, the wire payload, catch-up.

**Unlocks:** memory stops growing with conversation length. This is the phase
that carries semantic risk — an unread count computed by a query has to agree
exactly with one computed by a scan, or badges lie.

> **Phase 3 shipped 2026-08-22.** `Thread::wire_value_page` bounds the
> uncursored first load at `DEFAULT_THREAD_PAGE` (60), `thread.page` walks
> backward, and the SPA's gap check became the contiguity test this spec
> predicted it would have to. Three critical defects were caught in validation
> before release, all in the client cache: a forward delta pulling a
> below-window item into the window (`d6c1c3f`), a repaint after reset sticking
> a one-item conversation (`c6e061b`), and an in-flight page merging into a
> window that had been replaced (`f5faf88`).

### Phase 3 — wire pagination

- `thread.revision` and the detail polls gain a backward page: `before_sequence`
  + `limit`, alongside the `after_sequence` forward cursor that already exists.
- **The SPA's self-heal check has to change.** `createThreadCache` treats
  `merged.length !== thread_total` as a dropped delta and refetches the whole
  thread (`spa/src/core/thread.js:150`). Under pagination a client legitimately
  holds fewer items than the total, so that equality is no longer a gap signal —
  it has to become a contiguity check over sequences. This is small, and it will
  be silently wrong if it is missed: the symptom is a full refetch on every poll,
  which is the exact cost pagination was added to avoid.
- Scroll-back in the thread view.

**Unlocks:** bounded first load, and a conversation of any length over E2EE.

### Phase 4 — agent activity (from the harness spec)

With phases 1–3 done, activity events are just more `thread_items` rows with
`class = status`. `LIMIT`/`OFFSET` gives history without residency.

**This deletes the per-agent jsonl activity log** from the Agent Session spec.
That design existed only to route around the aggregate rewrite; with a database
it is a second store for no reason. `Agent Session Interface Spec.md` §6.2 is
superseded by this document.

---

## 5. Ordering against the harness work

The two workstreams are mostly independent, and they converge at exactly one
point.

| Harness step | Depends on the store? |
|---|---|
| 1 — `AgentSession` / `TerminalView` traits | No |
| 2 — status behind `status()` | No |
| 3 — optional terminal, `has_terminal`, typed refusals | No |
| **4 — activity kinds** | **Yes** — build this before the store and you build the jsonl log, then throw it away |
| 5 — SPA renders activity | Yes, via 4 |
| 6 — a provider with no terminal | Via 4 |

**Recommended order:** harness 1–3 (independent, and step 2 removes the "quiet
for 30 seconds means idle" guess regardless) → store 1–3 → harness 4–6.

The one thing not to do is harness step 4 before store phase 1: it means
building the jsonl log, shipping the write amplification it was designed to
avoid for everything *except* activity, and then unwinding both.

---

## 6. Risks

1. **The migration must be reversible.** JSON is renamed, not deleted, and the
   DB must be rebuildable from it. A corrupt DB with the source records gone is
   the user's work lost — the store already fails boot fast on a corrupt record
   for exactly this reason (`store.rs:47`).
2. **Unread counts are the sharp edge of phase 2.** They currently come from a
   scan and would come from a query. Any disagreement shows up as a badge that
   lies, which erodes trust in the rail faster than a slow poll does.
3. **`rusqlite` adds a C dependency.** `bundled` avoids a system library and
   links statically. Worth confirming against the release build before phase 1
   rather than after.
4. **Test surface.** 539 tests across `store.rs`, `thread.rs` and `app.rs` touch
   this area. Most are behavioural and should survive unchanged — that is the
   point of the `Store` boundary — but the 71 in `store.rs` describe the file
   layout directly.
5. **Do not conflate this with the app mutex.** SQLite offers real transactions
   and it will be tempting to reduce how long the app-wide lock is held. That is
   a separate change with its own failure modes; the store swap should be
   behaviour-preserving.

---

## 7. Revision history

- **2026-08-21, first draft.** Written after the JSON store's whole-aggregate
  rewrite was measured at 593 KB per state transition, and after "everything is
  files + git + PTY" was correctly identified as a statement about the user's
  surfaces rather than a persistence strategy.
