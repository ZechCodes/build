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
device/inode identity and usable creation metadata. If either cannot be
established, the bridge refuses the paged read with the existing `unavailable`
error; it emits no ambiguous cursor and does not fall back to an unpaged read.
Unopted legacy directory reads retain their existing behavior.
A cursor for another path, a removed/replaced directory, a damaged
token or an earlier bridge process is refused as `invalid_params`; it never
silently becomes a first page. Existing scope containment and source-binding
fences still apply. The filesystem worker scans outside the application lock.

Files paints held listings and selected bodies before asking for a current
first page. Each partial directory offers **Load more**. Only that action
requests its continuation. Appending a page retains the selected path and
reads the resulting cache record before repainting. A selected file can remain
open when its row is outside the loaded directory pages; absence from a
partial listing does not mean deletion. Reopening/revalidating the directory
starts a fresh first page, so saved cursors do not authorize reads after a
capability downgrade. The host-path browser uses `fs.list`, and its path
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

A file hint marks affected cached directories stale. Visible Files reads only
the touched open directory's first page; folded directories stay cold. A
truncated hint marks every cached directory in that worktree stale and reads
one visible directory: the selected item's parent when loaded and shown,
otherwise the root. Other stale listings paint from cache until opened.
No cursor continuation is followed by a hint. A selected file body refreshes
only when that path was named, or when the hint was truncated.

Run/worktree roots and exact original-project-source scopes receive live file
hints. A secondary workspace source has no change entity/producer in the
existing bridge. It retains the board/state wake fallback; opening the source
and reconnecting revalidate its visible data, but this fallback cannot promise
a live event for an external edit. This change does not invent a workspace
source producer or silently subscribe to a nonexistent entity.
