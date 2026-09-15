//! Splitting one envelope across a DataChannel's message limit, and putting it
//! back together.
//!
//! Pure: no channel, no I/O, no clock. The wire shape it hides —
//! `{"part":{"id","index","count"},"data":…}` — is the one fact this codebase
//! states twice, here and in `spa/src/core/chunk.js`, with the spec's table as
//! the source both are written from.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// The largest slice of an envelope one DataChannel message carries. Browsers
/// cap a single message far above this (Chrome at 256 KiB) and the
/// SDP-negotiated `a=max-message-size` is what actually binds, so this is the
/// conservative floor every implementation clears rather than a measured
/// maximum. An envelope at or under it crosses whole.
pub const CHUNK_BYTES: usize = 16 * 1024;

/// The largest envelope a reassembly may add up to. Its own number, deliberately:
/// application traffic rides the DataChannel and nothing else, so this cap is set
/// by what a peer may spend of the device's memory, not by any relay limit. Past
/// it the parts are an abuse, and the channel closes.
pub const MAX_REASSEMBLED_BYTES: usize = 8 * 1024 * 1024;

/// How a receiver tells a part from a whole envelope: the wrapper names `part`
/// first and an envelope never does. Written here, checked by
/// [`a_part_is_told_from_an_envelope_by_the_key_it_leads_with`].
const PART_PREFIX: &str = "{\"part\":";

/// A reassembly that cannot be finished. Always fatal to the channel it arrived
/// on: parts carry no way to ask for one again, so a reassembly that lost one
/// can only be started over on a fresh channel.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ChunkError {
    #[error("part {index} of message {id} arrived where part {expected} was due")]
    OutOfOrder {
        id: u64,
        index: usize,
        expected: usize,
    },
    #[error("a message past the {MAX_REASSEMBLED_BYTES} byte reassembly limit")]
    TooLarge,
    #[error("a part that is not one: {0}")]
    Malformed(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct Part {
    id: u64,
    index: usize,
    count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct PartMessage {
    part: Part,
    data: String,
}

/// One envelope as the messages that carry it: itself, when it fits, else its
/// ordered parts. Every part of one envelope shares an id, so a receiver that
/// is handed a part of another message knows before it appends.
pub fn split(envelope_json: &str) -> Vec<String> {
    if envelope_json.len() <= CHUNK_BYTES {
        return vec![envelope_json.to_string()];
    }
    static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_MESSAGE_ID.fetch_add(1, Ordering::Relaxed);
    let slices = slices(envelope_json);
    let count = slices.len();
    slices
        .into_iter()
        .enumerate()
        .map(|(index, data)| {
            serde_json::to_string(&PartMessage {
                part: Part { id, index, count },
                data: data.to_string(),
            })
            .expect("a part of a JSON string serializes")
        })
        .collect()
}

/// The envelope cut at char boundaries, no slice past [`CHUNK_BYTES`]. A
/// session id or a route may be any text, so a cut that lands mid-character
/// would hand the wire bytes that are not a string.
fn slices(text: &str) -> Vec<&str> {
    let mut slices = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + CHUNK_BYTES).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        slices.push(&text[start..end]);
        start = end;
    }
    slices
}

/// One channel's incoming messages, put back together.
///
/// **Hides** the wrapper and the partial buffer. **Interface** one verb: hand
/// it what arrived and it answers with an envelope, with nothing yet, or with
/// the error that closes the channel.
#[derive(Debug, Default)]
pub struct Reassembler {
    pending: Option<Pending>,
}

#[derive(Debug)]
struct Pending {
    id: u64,
    count: usize,
    next_index: usize,
    envelope: String,
}

impl Reassembler {
    /// The envelope this message completes, if it completes one.
    pub fn accept(&mut self, text: &str) -> Result<Option<String>, ChunkError> {
        if !text.starts_with(PART_PREFIX) {
            return match self.pending.take() {
                Some(pending) => Err(ChunkError::OutOfOrder {
                    id: pending.id,
                    index: pending.count,
                    expected: pending.next_index,
                }),
                None => Ok(Some(text.to_string())),
            };
        }
        let message: PartMessage =
            serde_json::from_str(text).map_err(|e| ChunkError::Malformed(e.to_string()))?;
        let part = message.part;
        if part.count == 0 || part.index >= part.count {
            return Err(ChunkError::Malformed(format!(
                "part {} of {}",
                part.index, part.count
            )));
        }
        let mut pending = self.due(&part)?;
        if pending.envelope.len() + message.data.len() > MAX_REASSEMBLED_BYTES {
            self.pending = None;
            return Err(ChunkError::TooLarge);
        }
        pending.envelope.push_str(&message.data);
        pending.next_index += 1;
        if pending.next_index == pending.count {
            self.pending = None;
            return Ok(Some(pending.envelope));
        }
        self.pending = Some(pending);
        Ok(None)
    }

    /// The reassembly this part continues, or the error that ends the channel.
    /// A part out of its sender's order, or one of another message while a
    /// reassembly is open, is the same fault: what was being assembled can no
    /// longer be completed.
    fn due(&mut self, part: &Part) -> Result<Pending, ChunkError> {
        match self.pending.take() {
            Some(pending) if pending.id == part.id && pending.next_index == part.index => {
                Ok(pending)
            }
            Some(pending) => Err(ChunkError::OutOfOrder {
                id: part.id,
                index: part.index,
                expected: pending.next_index,
            }),
            None if part.index == 0 => Ok(Pending {
                id: part.id,
                count: part.count,
                next_index: 0,
                envelope: String::new(),
            }),
            None => Err(ChunkError::OutOfOrder {
                id: part.id,
                index: part.index,
                expected: 0,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope_of(bytes: usize) -> String {
        "e".repeat(bytes)
    }

    fn reassemble(parts: &[String]) -> Result<Option<String>, ChunkError> {
        let mut reassembler = Reassembler::default();
        let mut completed = None;
        for part in parts {
            completed = reassembler.accept(part)?;
        }
        Ok(completed)
    }

    #[test]
    fn an_envelope_that_fits_crosses_whole() {
        let envelope = envelope_of(CHUNK_BYTES);

        let parts = split(&envelope);

        assert_eq!(parts, vec![envelope.clone()]);
        assert_eq!(reassemble(&parts), Ok(Some(envelope)));
    }

    #[test]
    fn one_byte_past_the_limit_is_two_parts_that_round_trip() {
        let envelope = envelope_of(CHUNK_BYTES + 1);

        let parts = split(&envelope);

        assert_eq!(parts.len(), 2);
        assert_eq!(reassemble(&parts), Ok(Some(envelope)));
    }

    #[test]
    fn every_part_of_a_large_envelope_fits_one_message() {
        let envelope = envelope_of(CHUNK_BYTES * 7 + 13);

        let parts = split(&envelope);

        assert_eq!(parts.len(), 8);
        for part in &parts {
            let carried: PartMessage = serde_json::from_str(part).expect("a part is JSON");
            assert!(carried.data.len() <= CHUNK_BYTES, "no slice is oversized");
        }
        assert_eq!(reassemble(&parts), Ok(Some(envelope)));
    }

    #[test]
    fn a_part_is_told_from_an_envelope_by_the_key_it_leads_with() {
        let parts = split(&envelope_of(CHUNK_BYTES + 1));

        for part in &parts {
            assert!(
                part.starts_with(PART_PREFIX),
                "{part} leads with the wrapper"
            );
        }
    }

    #[test]
    fn an_envelope_is_cut_between_characters_never_through_one() {
        let envelope = "é".repeat(CHUNK_BYTES);

        let parts = split(&envelope);

        assert_eq!(reassemble(&parts), Ok(Some(envelope)));
    }

    #[test]
    fn a_reassembly_answers_with_nothing_until_its_last_part() {
        let parts = split(&envelope_of(CHUNK_BYTES * 2));
        let mut reassembler = Reassembler::default();

        assert_eq!(reassembler.accept(&parts[0]), Ok(None));
        assert!(matches!(reassembler.accept(&parts[1]), Ok(Some(_))));
    }

    #[test]
    fn a_missing_part_ends_the_channel() {
        let parts = split(&envelope_of(CHUNK_BYTES * 3));
        let mut reassembler = Reassembler::default();
        reassembler.accept(&parts[0]).unwrap();

        assert!(matches!(
            reassembler.accept(&parts[2]),
            Err(ChunkError::OutOfOrder {
                index: 2,
                expected: 1,
                ..
            })
        ));
    }

    #[test]
    fn a_reassembly_that_starts_mid_message_ends_the_channel() {
        let parts = split(&envelope_of(CHUNK_BYTES * 2));

        assert!(matches!(
            Reassembler::default().accept(&parts[1]),
            Err(ChunkError::OutOfOrder { expected: 0, .. })
        ));
    }

    /// One ordered channel cannot interleave two messages, so a part of another
    /// message arriving mid-reassembly is a sender that is not the one this
    /// reassembly began with. It fails the same way a gap does.
    #[test]
    fn a_part_of_another_message_ends_the_channel() {
        let mine = split(&envelope_of(CHUNK_BYTES * 2));
        let theirs = split(&envelope_of(CHUNK_BYTES * 2));
        let mut reassembler = Reassembler::default();
        reassembler.accept(&mine[0]).unwrap();

        assert!(matches!(
            reassembler.accept(&theirs[1]),
            Err(ChunkError::OutOfOrder { .. })
        ));
    }

    #[test]
    fn a_whole_envelope_arriving_mid_reassembly_ends_the_channel() {
        let parts = split(&envelope_of(CHUNK_BYTES * 2));
        let mut reassembler = Reassembler::default();
        reassembler.accept(&parts[0]).unwrap();

        assert!(matches!(
            reassembler.accept("{\"version\":1}"),
            Err(ChunkError::OutOfOrder { .. })
        ));
    }

    #[test]
    fn a_reassembly_past_the_frame_cap_ends_the_channel() {
        let half = MAX_REASSEMBLED_BYTES / 2;
        let oversized: Vec<String> = (0..3)
            .map(|index| {
                serde_json::to_string(&PartMessage {
                    part: Part {
                        id: 9,
                        index,
                        count: 3,
                    },
                    data: envelope_of(half),
                })
                .unwrap()
            })
            .collect();

        assert_eq!(reassemble(&oversized), Err(ChunkError::TooLarge));
    }

    #[test]
    fn a_part_that_is_not_one_ends_the_channel() {
        let mut reassembler = Reassembler::default();

        assert!(matches!(
            reassembler.accept("{\"part\":{\"id\":1},\"data\":\"x\"}"),
            Err(ChunkError::Malformed(_))
        ));
        assert!(matches!(
            reassembler.accept("{\"part\":{\"id\":1,\"index\":0,\"count\":0},\"data\":\"x\"}"),
            Err(ChunkError::Malformed(_))
        ));
    }
}
