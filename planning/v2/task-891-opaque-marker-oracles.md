# Task #891 opaque marker oracles

The pure `spa/src/core/taskOpaqueMarkers.js` model is an isolated source oracle.
It does not replace the current unread adapters, change cache writes, or activate
a new unread/watch policy. Its tests retain independent expectations for the
current SPA and the canonical bridge semantics. Any migration of existing unread
or watch behavior needs an explicit scope/code decision and Astra disposition;
this document records differences for that review, not approval to activate them.

## Canonical source and interface

`bridge/src/tracker/summary.rs` defines `when` as the complete remainder after
the first hyphen, or the complete input when no hyphen exists. `after` compares
`when(id) > when(mark)` when a mark exists. It does not compare an extracted
suffix with a raw marker. `bridge/src/app/tracker/tracking.rs` treats a read as
already covered when `when(read) >= when(event_id)`. Both inputs to the new
comparison helpers must therefore be complete marker IDs. Re-extracting a
suffix that contains a hyphen would change its meaning.

The model exposes:

- `isPresentTaskMarker(value)`: every string is present, including `""`.
- `taskMarkerSuffix(id)`: one explicit first-hyphen extraction.
- `compareTaskMarkers(left, right)`: returns -1, 0, or 1 for optional complete
  markers; null/undefined absence sorts before any string.
- `taskMarkerAfter(mark, id)`: strict source `after` semantics; absence admits
  every entry, including an entry whose suffix is empty.
- `taskMarkerCovers(mark, id)`: a present read marker covers an entry at or
  below its suffix.
- `newerTaskMarker(left, right)`: preserves a complete input, retaining left
  on equal suffixes and returning null for two absent inputs. This selection
  convention does not assert that equal suffixes identify the same record.
- `effectiveTaskWatchFloor(task, timeline)`: one task's stored watch marker, or
  the last maximal watched wire event (`id`) when that stored marker is absent.
  It does not reconcile competing cache records or consume rendered row keys.

Markers are opaque, valid Unicode strings within caller-owned admission bounds.
The model applies no prefix restriction, ULID parsing, trimming, case folding,
NUL filtering, normalization, or length limit. Focused fixtures stay within 128
UTF-8 bytes, including an exact 128-byte marker. Rust `str` compares UTF-8 bytes;
JavaScript compares UTF-16 code units, which reverses some supplementary/BMP
orders. The model compares UTF-8 bytes and rejects JS-only unpaired surrogates
instead of silently replacing them during encoding.

## Visible behavior differences requiring separate disposition

| Input or situation | Current SPA oracle | Canonical source oracle | Potential visible effect if activated |
| --- | --- | --- | --- |
| Unknown prefix or marker without a hyphen | `trackerUnread.taskUnreadKey` rejects it; attention may ignore the read marker | Preserve and compare its source suffix | Unread lines, counts, and attention reasons may change |
| Empty marker or empty suffix | Unread key extraction rejects it; attention's truthiness admits all after an ignored marker | Empty string is present; equal empty suffixes are read | A previously shown unread entry or attention reason may clear |
| Multiple hyphens | Unread key extraction rejects it; attention extracts at most one segment and can discard the rest | Compare the complete first-hyphen remainder | New entries may become unread or older entries may be covered |
| Supplementary scalar versus high BMP scalar | Both current adapters use JS UTF-16 ordering | Rust UTF-8 lexical ordering | Read coverage and newest-marker selection may reverse |
| Present empty or unrecognized stored watch floor | Current watch helper falls back to a recognized watched event | `Some("")` and every other present string bypass fallback | The watch boundary and unread count may change |
| Equal-suffix watched events | Current helper retains the first recognized event | Rust `max_by_key` retains the last maximal event | Returned floor ID changes; equal suffix coverage stays equal |

NUL, whitespace, decomposed Unicode, and embedded hyphens remain significant
bytes. No migration may silently collapse these values into absence. Legacy
`ic-`/`ie-` renaming already produces the same extracted suffix for ordinary
markers; the canonical model needs no rename operation.

The focused test uses literal current-SPA expectations, a separate Buffer UTF-8
source oracle, and source-text guards for Rust `when`, `after`, watch fallback,
and inclusive read-through. This requires no bridge launch, full test suite,
new RPC, durable synchronization, or network change.
