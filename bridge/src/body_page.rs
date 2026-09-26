//! One page of a body too large to carry whole (#95).
//!
//! A body read (`fs.read`, `git.diff`, `git.show`, `git.changeset_diff`) that
//! names a `range` answers the bytes from `range.offset`, at most
//! `range.bytes` of them, and a [`BodySpan`] saying where they sit in the
//! whole. Offsets are bytes because the bridge can seek a file to one without
//! reading what comes before it, and because the whole body's size — the
//! file's, the patch's — is known up front. A page ends after the last line
//! end it holds, so each page is whole lines a client paints on its own; only
//! a line longer than a page is cut, at a character boundary. The next page
//! starts at the span's `end`. That is all the bridge does with a body here:
//! cut it where it was asked to.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most one page may carry. The wire cap of a whole-body read, so a page
/// never weighs more than the answer it replaces.
pub const BODY_PAGE_MAX_BYTES: u64 = 1_048_576;

/// The least one page carries, whatever was asked: a page always moves the
/// cursor on by whole lines unless one line is longer than this.
pub const BODY_PAGE_MIN_BYTES: u64 = 4_096;

/// Which bytes of a body one read asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BodyRange {
    /// Where the page starts: 0, or the `end` of the page before it.
    pub offset: u64,
    /// The most the page may carry, clamped to
    /// [`BODY_PAGE_MIN_BYTES`]`..=`[`BODY_PAGE_MAX_BYTES`].
    pub bytes: u64,
}

/// Where one page's bytes sit in the whole body.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct BodySpan {
    /// The first byte the page carries.
    pub offset: u64,
    /// One past the last byte it carries: the next page's `offset`. Equal to
    /// `total` on the last page.
    pub end: u64,
    /// The whole body's size in bytes.
    pub total: u64,
    /// Names the body the page was cut from: `fs.read` answers the file's
    /// modification time and size, a patch a digest of the whole patch text.
    /// Pages whose versions differ are cut from different bodies — a file's
    /// content key does not say that of its patch, which moves with HEAD and
    /// the merge base while the file stays as it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl BodyRange {
    /// The `range` param, when the read names one.
    pub fn from_params(params: &Value) -> Result<Option<Self>, String> {
        match params.get("range") {
            None | Some(Value::Null) => Ok(None),
            Some(range) => serde_json::from_value(range.clone())
                .map(Some)
                .map_err(|error| format!("invalid range: {error}")),
        }
    }

    /// How many bytes this page may carry.
    pub fn capacity(&self) -> usize {
        self.bytes.clamp(BODY_PAGE_MIN_BYTES, BODY_PAGE_MAX_BYTES) as usize
    }
}

/// How much of `window` — the bytes from a page's offset, at most its
/// capacity — the page keeps. All of it when the window reaches the body's
/// end; otherwise up to and including its last line end; otherwise, for a
/// line longer than the window, up to the last character boundary.
pub fn page_len(window: &[u8], reaches_end: bool) -> usize {
    if reaches_end {
        return window.len();
    }
    if let Some(newline) = window.iter().rposition(|byte| *byte == b'\n') {
        return newline + 1;
    }
    let cut = complete_characters(window);
    if cut == 0 {
        window.len()
    } else {
        cut
    }
}

/// The length of `window` without a UTF-8 sequence its last bytes begin and
/// do not finish. Bytes that are not UTF-8 are left as they are.
fn complete_characters(window: &[u8]) -> usize {
    let len = window.len();
    for back in 1..=len.min(4) {
        let byte = window[len - back];
        if byte & 0b1100_0000 == 0b1000_0000 {
            continue; // a continuation byte: its lead is further back
        }
        let width = match byte {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => return len,
        };
        return if width > back { len - back } else { len };
    }
    len
}

/// One page of a body held as text: the page and its span.
///
/// An offset past the end answers an empty page at the end, so a client that
/// asks on from a body that has since shrunk sees `total` move rather than an
/// error. An offset inside a character — no page of this text ends there, so
/// it is the end of a page of another — answers the page from the start of
/// that character: the version it carries is how the client learns the body
/// moved, and it reads the new one from the top.
pub fn text_page(text: &str, range: BodyRange) -> (String, BodySpan) {
    let total = text.len();
    let mut offset = usize::try_from(range.offset)
        .unwrap_or(usize::MAX)
        .min(total);
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let window_end = offset.saturating_add(range.capacity()).min(total);
    let len = page_len(&text.as_bytes()[offset..window_end], window_end == total);
    let end = offset + len;
    let page = text[offset..end].to_string();
    (
        page,
        BodySpan {
            offset: offset as u64,
            end: end as u64,
            total: total as u64,
            version: Some(text_version(text)),
        },
    )
}

/// A short digest naming one whole body of text.
fn text_version(text: &str) -> String {
    let mut digest = crate::scoped_file::revision_hex(text.as_bytes());
    digest.truncate(16);
    digest
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn range(offset: u64, bytes: u64) -> BodyRange {
        BodyRange { offset, bytes }
    }

    fn lines(count: usize, width: usize) -> String {
        (0..count)
            .map(|index| format!("{index:0width$}\n", width = width - 1))
            .collect()
    }

    #[test]
    fn a_read_without_a_range_names_none() {
        assert_eq!(BodyRange::from_params(&json!({ "path": "a" })), Ok(None));
        assert_eq!(BodyRange::from_params(&json!({ "range": null })), Ok(None));
    }

    #[test]
    fn a_range_is_an_offset_and_a_byte_count_and_nothing_else() {
        assert_eq!(
            BodyRange::from_params(&json!({ "range": { "offset": 8, "bytes": 4096 } })),
            Ok(Some(range(8, 4096)))
        );
        let unknown = BodyRange::from_params(&json!({
            "range": { "offset": 0, "bytes": 4096, "lines": 5 }
        }))
        .unwrap_err();
        assert!(unknown.contains("unknown field `lines`"), "{unknown}");
        let missing = BodyRange::from_params(&json!({ "range": { "offset": 0 } })).unwrap_err();
        assert!(missing.contains("missing field `bytes`"), "{missing}");
    }

    #[test]
    fn a_page_is_clamped_between_the_least_and_the_most_it_may_carry() {
        assert_eq!(range(0, 1).capacity(), BODY_PAGE_MIN_BYTES as usize);
        assert_eq!(range(0, u64::MAX).capacity(), BODY_PAGE_MAX_BYTES as usize);
        assert_eq!(range(0, 65_536).capacity(), 65_536);
    }

    #[test]
    fn a_page_ends_after_its_last_whole_line() {
        let text = lines(1000, 10); // 10 000 bytes of 10-byte lines
        let (page, span) = text_page(&text, range(0, 4_095 + 10));
        assert_eq!(page.len(), 4_100);
        assert!(page.ends_with('\n'));
        assert_eq!((span.offset, span.end, span.total), (0, 4_100, 10_000));
    }

    #[test]
    fn pages_read_on_from_each_end_cover_the_body_exactly_once() {
        let text = lines(3_001, 7);
        let mut offset = 0;
        let mut joined = String::new();
        let mut pages = 0;
        loop {
            let (page, span) = text_page(&text, range(offset, 4_096));
            assert_eq!(span.offset, offset);
            joined.push_str(&page);
            pages += 1;
            offset = span.end;
            if span.end == span.total {
                break;
            }
        }
        assert_eq!(joined, text);
        assert!(pages > 4, "{pages} pages");
    }

    #[test]
    fn the_last_page_keeps_a_final_line_with_no_line_end() {
        let text = format!("{}tail without newline", lines(10, 8));
        let (page, span) = text_page(&text, range(80, 4_096));
        assert_eq!(page, "tail without newline");
        assert_eq!(span.end, span.total);
    }

    #[test]
    fn a_line_longer_than_a_page_is_cut_at_a_character_boundary() {
        // 4095 ASCII bytes then a three-byte character straddling the cap.
        let text = format!("{}€€€ and on", "x".repeat(4_095));
        let (page, span) = text_page(&text, range(0, 4_096));
        assert_eq!(page.len(), 4_095);
        let (next, next_span) = text_page(&text, range(span.end, 4_096));
        assert!(next.starts_with('€'));
        assert_eq!(next_span.end, next_span.total);
    }

    #[test]
    fn every_page_of_one_text_names_it_and_another_text_differently() {
        let text = lines(3_000, 8);
        let first = text_page(&text, range(0, 4_096)).1.version;
        let later = text_page(&text, range(8_192, 4_096)).1.version;
        assert!(first.is_some());
        assert_eq!(first, later);
        let moved = format!("{text}one more\n");
        assert_ne!(text_page(&moved, range(0, 4_096)).1.version, first);
    }

    #[test]
    fn an_offset_past_the_end_answers_an_empty_last_page() {
        let (page, span) = text_page("short\n", range(9_000, 4_096));
        assert_eq!(page, "");
        assert_eq!((span.offset, span.end, span.total), (6, 6, 6));
    }

    /// The end of a page of an older text can fall inside a character of the
    /// new one (#95 round 2): the answer is still a page, from that
    /// character, under the new text's version, so the client sees the body
    /// moved rather than asking the same offset forever.
    #[test]
    fn an_offset_inside_a_character_answers_from_that_character_under_its_version() {
        let old = "x".repeat(8_192);
        let (_, old_span) = text_page(&old, range(0, 4_096));
        assert_eq!(old_span.end, 4_096);
        let new = format!("{}€ rest\n", "x".repeat(4_095));
        let (page, span) = text_page(&new, range(old_span.end, 4_096));
        assert_eq!(span.offset, 4_095);
        assert!(page.starts_with('€'), "{page}");
        assert_eq!(span.end, span.total);
        assert_ne!(span.version, old_span.version);
    }

    #[test]
    fn a_window_of_bytes_that_are_not_text_is_kept_whole() {
        let binary = [0xffu8; 16];
        assert_eq!(page_len(&binary, false), 16);
        let split = [b'a', 0xe2, 0x82];
        assert_eq!(page_len(&split, false), 1);
        assert_eq!(page_len(&split, true), 3);
    }
}
