use super::*;
use serde_json::json;

const FP: &str = "00:01:02:03:04:05:06:07:08:09:0A:0B:0C:0D:0E:0F:10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F";

fn sdp() -> String {
    format!(
        "v=0\r\na=fingerprint:sha-256 {FP}\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n"
    )
}

fn transcript() -> Transcript {
    Transcript::new(&sdp(), &sdp(), &fresh_token(), &fresh_token()).unwrap()
}

#[test]
fn transcript_requires_sha256_fingerprints_and_hashes_exact_original_sdp() {
    let sdp = sdp();
    assert!(Transcript::new("v=0", &sdp, &fresh_token(), &fresh_token()).is_err());
    assert!(Transcript::new(
        &sdp.replace("sha-256", "sha-1"),
        &sdp,
        &fresh_token(),
        &fresh_token()
    )
    .is_err());
    let original = transcript();
    let changed = Transcript::new(
        &(sdp.clone() + "\r\n"),
        &sdp,
        &original.peer_nonce,
        &original.peer_token,
    )
    .unwrap();
    assert_ne!(original.offer_hash, changed.offer_hash);
    assert_eq!(
        original.offer_fingerprint,
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
    );
}

#[test]
fn ambiguous_or_malformed_fingerprints_and_noncanonical_tokens_fail_closed() {
    let sdp = sdp();
    let ambiguous = sdp.replacen(
        "m=",
        &format!("a=fingerprint:sha-256 {}\r\nm=", FP.replace("00", "FF")),
        1,
    );
    assert!(fingerprint(&ambiguous).is_err());
    assert!(fingerprint(&sdp.replace(":1F", "")).is_err());
    assert!(Transcript::new(&sdp, &sdp, "x", &fresh_token()).is_err());
    assert!(Transcript::new(&sdp, &sdp, &(fresh_token() + "="), &fresh_token()).is_err());
}

#[test]
fn application_fingerprints_inherit_session_but_ignore_unrelated_media() {
    let app = sdp();
    let audio = format!(
        "m=audio 9 UDP/TLS/RTP/SAVPF 0\r\na=fingerprint:sha-256 {}\r\n",
        FP.replace("00", "FF")
    );
    assert_eq!(
        fingerprint(&(app + &audio)).unwrap(),
        transcript().offer_fingerprint
    );
    assert!(
        fingerprint(&format!("v=0\r\na=fingerprint:sha-256 {FP}\r\n")).is_err(),
        "no application media"
    );
    assert!(
        fingerprint(&sdp().replace("application 9", "application 0")).is_err(),
        "rejected section"
    );
}

#[test]
fn proof_compares_typed_fields_and_uses_distinct_ack_domain() {
    let expected = BindProof::new(
        "app-session",
        &transcript(),
        ChannelRole::App,
        &fresh_token(),
    )
    .unwrap();
    let key = crate::transport::generate_session_key();
    let envelope = expected.seal(&key, false, None).unwrap();
    expected.verify(&key, &envelope, false).unwrap();
    assert!(
        expected.verify(&key, &envelope, true).is_err(),
        "proof cannot be reflected as an ack"
    );
    assert!(expected
        .verify(&crate::transport::generate_session_key(), &envelope, false)
        .is_err());
    let mut other = expected.clone();
    other.channel_id = 1;
    other.channel_label = "term".into();
    assert!(other.verify(&key, &envelope, false).is_err());
    other = expected.clone();
    other.challenge = fresh_token();
    assert!(other.verify(&key, &envelope, false).is_err());
    let binding_id = fresh_token();
    let ack = expected.seal(&key, true, Some(&binding_id)).unwrap();
    assert_eq!(expected.verify(&key, &ack, true).unwrap(), Some(binding_id));
}

#[test]
fn every_captured_field_and_envelope_session_must_match() {
    let expected = BindProof::new(
        "app-session",
        &transcript(),
        ChannelRole::App,
        &fresh_token(),
    )
    .unwrap();
    let key = crate::transport::generate_session_key();
    for field in [
        "session_id",
        "peer_nonce",
        "peer_token",
        "offer_hash",
        "answer_hash",
        "offer_fingerprint",
        "answer_fingerprint",
        "protocol",
        "challenge",
        "channel_label",
    ] {
        let mut forged = serde_json::to_value(&expected).unwrap();
        forged[field] = json!("wrong");
        let forged: BindProof = serde_json::from_value(forged).unwrap();
        let envelope = forged.seal(&key, false, None).unwrap();
        assert!(expected.verify(&key, &envelope, false).is_err(), "{field}");
    }
    let mut envelope = expected.seal(&key, false, None).unwrap();
    envelope.session_id = "other".into();
    assert!(expected.verify(&key, &envelope, false).is_err());
}

#[test]
fn clear_payloads_have_no_cipher_and_only_exact_sequences_are_accepted() {
    let id = fresh_token();
    let mut writer = ClearCodec::new(&id).unwrap();
    let mut reader = ClearCodec::new(&id).unwrap();
    let payload = json!({"id": 1, "method":"ping", "params":{}});
    let first = writer.encode("data", payload.clone()).unwrap();
    assert_eq!(first["payload"], payload);
    assert!(first.get("ciphertext").is_none());
    assert_eq!(reader.decode(first.clone()).unwrap().payload, payload);
    assert!(reader.decode(first).is_err(), "duplicate is fatal");
    assert!(
        reader
            .decode(writer.encode("data", json!({})).unwrap())
            .is_err(),
        "failed codec stays closed"
    );
}

#[test]
fn clear_codec_rejects_wrong_binding_gaps_unsafe_numbers_and_invalid_frame_types() {
    let id = fresh_token();
    let first = ClearCodec::new(&id)
        .unwrap()
        .encode("data", json!({}))
        .unwrap();
    for (field, value) in [
        ("binding_id", json!(fresh_token())),
        ("seq", json!(2)),
        ("seq", json!(0)),
        ("seq", json!(1.5)),
        ("seq", json!(MAX_SEQUENCE + 1)),
        ("frame_type", json!("rtc.offer")),
        ("protocol", json!("secretbox-v1")),
    ] {
        let mut forged = first.clone();
        forged[field] = value;
        assert!(
            ClearCodec::new(&id).unwrap().decode(forged).is_err(),
            "{field}"
        );
    }
    let mut codec = ClearCodec::new(&id).unwrap();
    assert!(codec.encode("close", json!({})).is_ok());
    assert!(codec.encode("data", json!({})).is_err());
}

#[test]
fn protocol_selection_is_explicit_and_legacy_only_before_commitment() {
    assert_eq!(
        PayloadProtocol::select(None).unwrap(),
        PayloadProtocol::Secretbox
    );
    assert_eq!(
        PayloadProtocol::select(Some(&json!(["dtls-json-v1", "secretbox-v1"]))).unwrap(),
        PayloadProtocol::DtlsJson
    );
    assert_eq!(
        PayloadProtocol::select(Some(&json!(["secretbox-v1"]))).unwrap(),
        PayloadProtocol::Secretbox
    );
    assert!(PayloadProtocol::select(Some(&json!(["unsupported"]))).is_err());
    assert!(PayloadProtocol::select(Some(&json!("dtls-json-v1"))).is_err());
}

#[test]
fn js_proof_and_ack_fixture_authenticate_with_rust_without_reserialization_rules() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../../fixtures/api/rtc-binding-v1.json")).unwrap();
    let transcript = Transcript::new(
        fixture["offer_sdp"].as_str().unwrap(),
        fixture["answer_sdp"].as_str().unwrap(),
        fixture["transcript"]["peer_nonce"].as_str().unwrap(),
        fixture["transcript"]["peer_token"].as_str().unwrap(),
    )
    .unwrap();
    let expected = BindProof::new(
        fixture["session_id"].as_str().unwrap(),
        &transcript,
        ChannelRole::App,
        fixture["challenge"].as_str().unwrap(),
    )
    .unwrap();
    let key = fixture["test_session_key"].as_str().unwrap();
    let proof = serde_json::from_value(fixture["proof"]["envelope"].clone()).unwrap();
    let ack = serde_json::from_value(fixture["ack"]["envelope"].clone()).unwrap();
    expected.verify(key, &proof, false).unwrap();
    assert_eq!(
        expected.verify(key, &ack, true).unwrap().unwrap(),
        fixture["binding_id"].as_str().unwrap()
    );
}
