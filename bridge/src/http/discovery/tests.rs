use super::*;
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use base64::Engine;

fn identity() -> crate::identity::StoredIdentity {
    let pair = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
    serde_json::from_value(serde_json::json!({
        "device_id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "name":"test fixture",
        "transport":{"private_key_b64":"","public_key_b64":""},
        "identity_private_key_b64": base64::engine::general_purpose::STANDARD.encode([7; 32]),
        "identity_public_key_b64": base64::engine::general_purpose::STANDARD.encode(pair.public_key().as_ref()),
        "approved": true,
        "approved_by": "https://getbuild.ing"
    })).unwrap()
}

#[test]
fn origins_are_https_and_canonical_without_url_aliases() {
    for (input, expected) in [
        ("https://EXAMPLE.com:443/", "https://example.com"),
        ("https://[::1]:8443/", "https://[::1]:8443"),
        ("https://bücher.example", "https://xn--bcher-kva.example"),
    ] {
        assert_eq!(CanonicalOrigin::parse(input).unwrap().as_str(), expected);
    }
    for bad in [
        "http://localhost",
        " https://a",
        "https://a/b",
        "https://a/..",
        "https://a?x",
        "https://a#x",
        "https://u@a",
        "https://a.",
        "https://127.1",
        "https://0x7f000001",
        "https://[fe80::1%25eth0]",
        "https://a:0",
        "https://a\\b",
        "https://%61.example",
    ] {
        assert!(CanonicalOrigin::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn unapproved_and_other_issuer_cannot_sign() {
    let mut id = identity();
    id.approved = false;
    assert!(DeviceProofSigner::new(
        &id,
        "https://getbuild.ing",
        CanonicalOrigin::parse("https://bridge.example").unwrap(),
        "epoch-1".into()
    )
    .is_err());
    id.approved = true;
    assert!(DeviceProofSigner::new(
        &id,
        "https://dev.getbuild.ing",
        CanonicalOrigin::parse("https://bridge.example").unwrap(),
        "epoch-1".into()
    )
    .is_err());
}

#[test]
fn proof_has_fixed_origin_audience_purpose_and_fresh_jti() {
    let id = identity();
    let signer = DeviceProofSigner::new(
        &id,
        "https://getbuild.ing",
        CanonicalOrigin::parse("https://bridge.example").unwrap(),
        "epoch-1".into(),
    )
    .unwrap();
    let first = signer
        .discovery_proof("challenge-1", "a-long-random-nonce", 1_800_000_000)
        .unwrap();
    let second = signer
        .discovery_proof("challenge-1", "a-long-random-nonce", 1_800_000_000)
        .unwrap();
    let h = jsonwebtoken::decode_header(&first).unwrap();
    assert_eq!(h.alg, jsonwebtoken::Algorithm::EdDSA);
    assert_eq!(h.typ.as_deref(), Some("build-http-discovery+jwt"));
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::EdDSA);
    validation.validate_exp = false;
    validation.set_audience(&["https://getbuild.ing/api/bridge-discovery/complete"]);
    let public = base64::engine::general_purpose::STANDARD
        .decode(id.identity_public_key_b64)
        .unwrap();
    let decode = |proof: &str| {
        jsonwebtoken::decode::<serde_json::Value>(
            proof,
            &jsonwebtoken::DecodingKey::from_ed_der(&public),
            &validation,
        )
        .unwrap()
        .claims
    };
    let claims = decode(&first);
    assert_eq!(claims["origin"], "https://bridge.example");
    assert_eq!(claims["issuer"], "https://getbuild.ing");
    assert_eq!(claims["endpoint_epoch"], "epoch-1");
    assert_eq!(
        claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(),
        30
    );
    assert_ne!(claims["jti"], decode(&second)["jti"]);
    assert!(signer
        .discovery_proof("", "a-long-random-nonce", 1)
        .is_err());
    assert!(signer.discovery_proof("a", &"x".repeat(2049), 1).is_err());
}

#[test]
fn registration_and_introspection_have_disjoint_purpose_and_token_binding() {
    let signer = DeviceProofSigner::new(
        &identity(),
        "https://getbuild.ing",
        CanonicalOrigin::parse("https://bridge.example").unwrap(),
        "epoch-2".into(),
    )
    .unwrap();
    let registration = signer
        .registration_proof("challenge", "nonce", Some("epoch-1"), false, 1_800_000_000)
        .unwrap();
    let assertion = signer
        .introspection_assertion(&"a".repeat(64), 1_800_000_000)
        .unwrap();
    assert_eq!(
        jsonwebtoken::decode_header(&registration)
            .unwrap()
            .typ
            .as_deref(),
        Some("build-http-registration+jwt")
    );
    assert_eq!(
        jsonwebtoken::decode_header(&assertion)
            .unwrap()
            .typ
            .as_deref(),
        Some("build-http-introspection+jwt")
    );
    assert!(signer.introspection_assertion("raw-token", 1).is_err());
    assert!(signer
        .discovery_proof("challenge", "nonce", u64::MAX)
        .is_err());
}

async fn registrar(server: &wiremock::MockServer) -> EndpointRegistrar {
    // Only the tests can substitute cleartext fixture transport. Production
    // constructors pin the paired HTTPS issuer and never expose this path.
    let mut signer = DeviceProofSigner::new(
        &identity(),
        "https://getbuild.ing",
        CanonicalOrigin::parse("https://bridge.example").unwrap(),
        "epoch-1".into(),
    )
    .unwrap();
    signer.issuer = CanonicalOrigin(server.uri());
    EndpointRegistrar {
        signer: std::sync::Arc::new(signer),
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap(),
    }
}

#[tokio::test]
async fn registration_uses_fixed_paths_and_validates_echoed_binding() {
    use wiremock::{
        matchers::{body_json, header, method, path},
        Mock, ResponseTemplate,
    };
    let server = wiremock::MockServer::start().await;
    let registrar = registrar(&server).await;
    let registration = format!(
        "/api/devices/{}/http-endpoint",
        registrar.signer.device_id()
    );
    Mock::given(method("POST")).and(path(format!("{registration}/challenge"))).and(header("content-type","application/json")).and(body_json(serde_json::json!({}))).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"challenge_id":"challenge","nonce":"nonce","expires_at":unix_now().unwrap()+60}))).mount(&server).await;
    Mock::given(method("POST")).and(path(&registration)).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"origin":"https://bridge.example","endpoint_epoch":"epoch-1","enabled":true}))).mount(&server).await;
    assert!(registrar.register(None, true).await.unwrap().enabled);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(!requests[1].headers.contains_key("authorization"));
    let body: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(body.as_object().unwrap().len(), 1);
    assert!(body["proof"].as_str().unwrap().len() < 16384);
    server.reset().await;
    Mock::given(method("POST")).and(path(format!("{registration}/challenge"))).and(header("content-type","application/json")).and(body_json(serde_json::json!({}))).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"challenge_id":"challenge","nonce":"nonce","expires_at":unix_now().unwrap()+60}))).mount(&server).await;
    Mock::given(method("POST")).and(path(&registration)).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"origin":"https://attacker.example","endpoint_epoch":"epoch-1","enabled":true}))).mount(&server).await;
    assert_eq!(
        registrar.register(None, true).await,
        Err(ProofError::Refused)
    );
}

#[tokio::test]
async fn registration_rejects_redirect_and_oversized_body_without_forwarding() {
    use wiremock::{matchers::path, Mock, ResponseTemplate};
    let server = wiremock::MockServer::start().await;
    let trap = wiremock::MockServer::start().await;
    let registrar = registrar(&server).await;
    Mock::given(path(format!(
        "/api/devices/{}/http-endpoint/challenge",
        registrar.signer.device_id()
    )))
    .respond_with(ResponseTemplate::new(307).insert_header("location", trap.uri()))
    .mount(&server)
    .await;
    assert_eq!(
        registrar.register(None, true).await,
        Err(ProofError::Refused)
    );
    assert!(trap.received_requests().await.unwrap().is_empty());
    server.reset().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(16385)))
        .mount(&server)
        .await;
    assert_eq!(
        registrar.register(None, true).await,
        Err(ProofError::Refused)
    );
}

#[test]
fn origins_reject_invalid_dns_labels_and_noncanonical_device_ids() {
    for bad in [
        "https://bridge_name.example",
        "https://-bridge.example",
        "https://bridge-.example",
    ] {
        assert!(CanonicalOrigin::parse(bad).is_err(), "{bad}");
    }
    let mut id = identity();
    id.device_id = "{aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa}".into();
    assert!(DeviceProofSigner::new(
        &id,
        "https://getbuild.ing",
        CanonicalOrigin::parse("https://bridge.example").unwrap(),
        "epoch-1".into()
    )
    .is_err());
}
