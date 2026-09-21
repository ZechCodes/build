use super::*;

fn reading(tokens: u64, window: Option<u64>) -> ContextReading {
    ContextReading {
        tokens,
        window,
        compact_at: None,
        at: "2026-09-21T12:00:00Z".to_string(),
    }
}

fn compacting_at(tokens: u64, compact_at: u64) -> ContextReading {
    ContextReading {
        compact_at: Some(compact_at),
        ..reading(tokens, Some(1_000_000))
    }
}

/// A chat that compacts is measured against where it compacts, not against
/// the model's window: 190k of 1M reads as room to spare one turn before
/// the compaction that says otherwise.
#[test]
fn a_reading_that_compacts_is_measured_against_its_threshold() {
    assert_eq!(
        compacting_at(190_000, 200_000).sentence("Rail scroll"),
        "Rail scroll is at 190k of 200k (95%, compacts at 200k)."
    );
    assert_eq!(
        compacting_at(210_000, 200_000).sentence("Rail scroll"),
        "Rail scroll is at 210k of 200k (105%, compacts at 200k)."
    );
}

#[test]
fn a_reading_that_never_compacts_falls_back_to_the_window() {
    assert_eq!(
        compacting_at(612_000, 0).sentence("Rail scroll"),
        "Rail scroll is at 612k of 1M (61%)."
    );
}

#[test]
fn a_reading_serializes_the_threshold_beside_the_honest_window() {
    assert_eq!(
        serde_json::to_value(compacting_at(190_000, 200_000)).unwrap(),
        serde_json::json!({
            "tokens": 190_000,
            "window": 1_000_000,
            "compact_at": 200_000,
            "at": "2026-09-21T12:00:00Z"
        })
    );
}

#[test]
fn a_reading_with_a_window_says_how_much_of_it_is_used() {
    assert_eq!(
        reading(612_000, Some(1_000_000)).sentence("Rail scroll"),
        "Rail scroll is at 612k of 1M (61%)."
    );
}

#[test]
fn a_reading_without_a_window_says_only_the_size() {
    assert_eq!(
        reading(612_000, None).sentence("Rail scroll"),
        "Rail scroll is at 612k."
    );
}

#[test]
fn thousands_round_to_k_and_millions_to_one_decimal_m() {
    assert_eq!(
        reading(1_200_000, Some(2_000_000)).sentence("Tracker"),
        "Tracker is at 1.2M of 2M (60%)."
    );
    assert_eq!(
        reading(148_210, Some(200_000)).sentence("Tracker"),
        "Tracker is at 148k of 200k (74%)."
    );
    assert_eq!(
        reading(999_700, None).sentence("Tracker"),
        "Tracker is at 1M.",
        "a count that rounds up to a thousand k is a million"
    );
    assert_eq!(reading(640, None).sentence("Tracker"), "Tracker is at 640.");
}

#[test]
fn a_reading_serializes_without_a_window_it_does_not_know() {
    assert_eq!(
        serde_json::to_value(reading(612_000, None)).unwrap(),
        serde_json::json!({ "tokens": 612_000, "at": "2026-09-21T12:00:00Z" })
    );
}
