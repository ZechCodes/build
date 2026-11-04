use super::*;

#[test]
fn unavailable_diagnostic_uses_exact_count_only_format() {
    for (count, expected) in [
        (0, "secret vault unavailable records: 0"),
        (2, "secret vault unavailable records: 2"),
        (1000, "secret vault unavailable records: 1000"),
    ] {
        let mut captured = Vec::new();
        emit_unavailable(count, |message| captured.push(message.to_owned()));
        assert_eq!(captured, vec![expected]);
        for forbidden in [
            "SYNTHETIC_PRIVATE_NAME",
            "sec_synthetic-private-id",
            "synthetic-device",
            "vault.key",
        ] {
            assert!(!captured[0].contains(forbidden));
        }
    }
}
