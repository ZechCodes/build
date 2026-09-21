use super::*;

fn reading(tokens: u64, window: Option<u64>) -> ContextReading {
    ContextReading {
        tokens,
        window,
        at: "2026-09-21T12:00:00Z".to_string(),
    }
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
