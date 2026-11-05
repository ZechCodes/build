# Directory pages (wire 3.35)

`fs.treePaged` advertises an opt-in `fs.tree` mode. Requests without `paged:true`
retain the existing directory listing and result shape. A capable client asks
for `{paged:true, limit:100}`; `limit` must be 1–200. The result adds `has_more`
and `next_cursor` (null on the last page). A continuation repeats the scope and
path and sends the returned cursor. There is no offset and no implicit drain.

Pages use directories first, then Unicode-lowercased filename, with the raw
filename as the deterministic tie breaker. Each scan considers entries after
the previous key and keeps at most `limit + 1` candidates. Directory scanning
is still O(n) per page, while candidate memory and response rows are bounded.
Deleting earlier entries does not shift the next page. As with a live keyset,
an entry inserted before the cursor is found by a new first-page read;
inserting or deleting before the cursor cannot shift existing later entries
out of the next page or repeat earlier entries.

Cursors are opaque authenticated tokens. They bind the canonical scope root,
requested path and directory identity. Paged reads require a nonzero Unix
device/inode identity. When creation metadata is available, the existing
birth-time identity and cursor format remain unchanged. When only creation
metadata is missing, the bridge holds the actual directory open in a bounded
lease: at most 32 retained handles, oldest-first eviction, and an absolute
120-second lifetime that access never extends. Its ID appears only inside
the authenticated opaque cursor. Each continuation compares the current
path with the held handle and scans relative to that handle, so replacement
cannot recycle the leased inode or substitute another directory's entries.
Expired or evicted leases refuse continuations with `invalid_params`; a new
first page remains available without an unbounded fallback. Idle expired
handles are reclaimed on the next registry operation or process exit.
Missing platform device/inode identity still refuses with `unavailable`.
Unopted legacy directory reads retain their existing behavior.
A cursor for another path, a removed/replaced directory, a damaged
token or an earlier bridge process is refused as `invalid_params`; it never
silently becomes a first page. Existing scope containment and source-binding
fences still apply. The filesystem worker scans outside the application lock.

Files paints held listings and selected bodies before asking for a current
first page. Each partial directory offers **Load more**. Appending a page
retains the selected path and records the bounded held-window depth as
`loaded_pages`. Older cached records without that field infer a page budget
from `ceil(entries.length / 100)`, still capped at 8. Refresh starts at page
one and rebuilds toward the previously held tail sort key (directory-first
bucket plus name), using at most the held page count capped at **8 pages per directory**.
The rebuilt window publishes atomically, so a selected row within that window
does not disappear during intermediate page reads. Reveal/deep-link demand
may also page toward its requested row, stopping once found, with a shared
maximum of **8 additional continuation reads** across the path's ancestors.
Neither operation drains the directory. A selected file can remain open when
its row lies outside the loaded pages; absence from a partial listing does
not mean deletion. Beyond the automatic budget, further continuation remains
explicit user demand. A current first-page read still starts every refresh,
so saved cursors never authorize requests after a capability downgrade. The host-path browser uses `fs.list`, and its path
completion is unaffected. Saved review Git trees retain their existing
immutable read contract; live review directories use the paged `fs.tree` mode.

`changes.filesHints` advertises the separate `files.hints` subscription kind.
It emits the existing `files:{paths,truncated}` object with no root listing.
Only a legacy `files` subscriber can cause root facts to be read, and a hint
subscriber never receives that body even when a legacy subscriber shares the
same flush. Names remain confined relative paths, capped at 200 per flush.
An invalid/oversized name makes the hint truncated instead of becoming an
untrusted cache address.
An older `files` frame with no named paths also means an unknown invalidation.
When the current bridge lacks `changes.filesHints`, a complete legacy root
listing already carried by the frame is written to the visible root cache
before repainting, clearing any old paging continuation. That fulfilled root
needs no duplicate RPC; touched visible deeper directories and selected files
still refresh. Folded roots retain stale cache without admitting the body or
reading. Greeting, visibility and cache-write fences guard root admission. A
hint-capable bridge never authorizes root admission from a `files` payload.

A file hint marks affected cached directories stale. Visible Files refreshes
each affected expanded directory's bounded held window; folded directories
stay cold. A truncated hint marks every cached directory in that worktree
stale and refreshes **all currently visible expanded directories**, including
unselected siblings. Folded descendants retain stale cached rows until opened.
A hint can therefore follow the bounded continuations needed to restore each
held window (at most 8 pages per directory), while hidden directories issue
no reads. A selected file body refreshes only when that path was named, or
when the hint was truncated.

Run/worktree roots and exact original-project-source scopes receive live file
hints. A secondary workspace source has no change entity/producer in the
existing bridge. It retains the board/state wake fallback; opening the source
and reconnecting revalidate its visible data, but this fallback cannot promise
a live event for an external edit. This change does not invent a workspace
source producer or silently subscribe to a nonexistent entity.

Live task-review Files reuses the same scope selector: when supported it
subscribes to `files.hints` under the exact original `project_source` identity.
Without that subject, its board registration retains explicit/reconnect
refresh only; ordinary state items, including unrelated workspace/agent
state, cause no directory or body reads. Its legacy workspace/files branch
remains unchanged. Hint reads capture watcher generation, visibility and
selected-file ownership so retired bindings cannot publish their late answers.

Absent-birth-time tests compose actual Rust directory I/O and the real
bounded lease registry with separate SPA public-response/refusal-code tests.
The SPA responses are scripted; they do not execute the Rust provider or
constitute an installed-bridge deployment.

With scoped ownership on a 3.34 bridge, `files` still carries a full root.
Its Files subscription is removed while that root is folded or the document
is hidden. Showing it again restores the subscription and paints held data
before refreshing visible first pages. Modern root-free hints can remain
subscribed to mark hidden caches stale. Binding generations retire in-flight
frames on unsubscribe or greeting changes, including hide/show races.
Scoped Git panes release body subscriptions while their pane or document is
hidden. Showing them restores the watcher and revalidates their owned
status/history/unpushed metadata from cache. A mounted visible review owns its
own diff refresh; a history-only pane does not warm working diffs. Selected
changesets alone read hunks. Non-scoped/legacy 3.33 behavior is unchanged.
