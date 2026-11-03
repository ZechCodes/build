use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};

const LOCAL: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const FOREIGN: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const KEY: [u8; 32] = [0x55; 32];

fn scope() -> Scope {
    Scope::new(Stream::device(), json!({"kind":"device"}))
}
fn raw(payload: &Value, tag: &[u8]) -> String {
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload).unwrap()),
        URL_SAFE_NO_PAD.encode(tag)
    )
}
fn foreign(scope: &Scope) -> Value {
    json!({"version":1,"purpose":"resume","incarnation":FOREIGN,"scope":scope.digest(),"sequence":"0"})
}

#[test]
fn f2_foreign_cursor_rejects_malformed_fields_before_incarnation_reset() {
    let scope = scope();
    let valid = foreign(&scope);
    assert!(matches!(
        cursor::decode(&raw(&valid, &[0; 32]), LOCAL, &KEY, &scope),
        Ok(cursor::Position::Foreign)
    ));
    let cases = [
        ("version", json!(2)),
        ("purpose", json!("snapshot")),
        ("incarnation", json!(FOREIGN.to_uppercase())),
        ("incarnation", json!(format!("{{{FOREIGN}}}"))),
        ("incarnation", json!("not-a-uuid")),
        ("scope", json!("abcdef")),
        ("scope", json!(scope.digest().to_uppercase())),
        ("scope", json!("g".repeat(64))),
        ("sequence", json!("00")),
        ("sequence", json!(0)),
        ("sequence", json!("-1")),
        ("sequence", json!("9223372036854775808")),
        ("unknown_field", json!(true)),
    ];
    for (field, value) in cases {
        let mut bad = valid.clone();
        bad[field] = value;
        assert!(
            matches!(
                cursor::decode(&raw(&bad, &[0; 32]), LOCAL, &KEY, &scope),
                Err(SyncError::MalformedCursor)
            ),
            "malformed foreign {field} must not become resync_required"
        );
    }
    assert!(matches!(
        cursor::decode(&raw(&valid, &[0; 31]), LOCAL, &KEY, &scope),
        Err(SyncError::MalformedCursor)
    ));
}

#[test]
fn f2_well_formed_foreign_cursor_still_refuses_a_different_scope() {
    let scope = scope();
    let other = Scope::new(Stream::project("synthetic-other"), json!({}));
    let token = raw(&foreign(&other), &[0; 32]);
    assert!(matches!(
        cursor::decode(&token, LOCAL, &KEY, &scope),
        Err(SyncError::ScopeMismatch)
    ));
}

#[test]
fn f2_signature_tampering_including_all_spare_bit_variants_is_rejected() {
    let scope = scope();
    let token = cursor::encode(LOCAL, &KEY, &scope, Sequence::ZERO);
    assert!(matches!(
        cursor::decode(&token, LOCAL, &KEY, &scope),
        Ok(cursor::Position::Retained(Sequence::ZERO))
    ));
    let (payload, tag) = token.split_once('.').unwrap();
    for index in [0, tag.len() - 2] {
        let mut bad = tag.as_bytes().to_vec();
        bad[index] = if bad[index] == b'A' { b'B' } else { b'A' };
        let token = format!("{payload}.{}", String::from_utf8(bad).unwrap());
        assert!(matches!(
            cursor::decode(&token, LOCAL, &KEY, &scope),
            Err(SyncError::MalformedCursor)
        ));
    }
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let last = tag.as_bytes()[tag.len() - 1];
    let canonical = alphabet.iter().position(|byte| *byte == last).unwrap();
    assert_eq!(canonical & 3, 0);
    for spare in 1..=3 {
        let mut bad = tag.as_bytes().to_vec();
        bad[tag.len() - 1] = alphabet[canonical | spare];
        let token = format!("{payload}.{}", String::from_utf8(bad).unwrap());
        assert!(
            matches!(
                cursor::decode(&token, LOCAL, &KEY, &scope),
                Err(SyncError::MalformedCursor)
            ),
            "noncanonical trailing bits {spare} were accepted"
        );
    }
}
