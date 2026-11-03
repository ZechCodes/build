use super::*;
use crate::http::discovery::{CanonicalOrigin, DeviceProofSigner};
use crate::http::listener::PeerAddress;
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use base64::{engine::general_purpose::STANDARD, Engine};

fn fixture() -> (HttpRuntime, Vec<u8>, Arc<AtomicUsize>) {
    let pair = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
    let public = pair.public_key().as_ref().to_vec();
    let identity = serde_json::from_value(json!({
        "device_id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", "name": "fixture",
        "transport": {"private_key_b64": "", "public_key_b64": ""},
        "identity_private_key_b64": STANDARD.encode([7; 32]),
        "identity_public_key_b64": STANDARD.encode(&public),
        "approved": true, "approved_by": "https://getbuild.ing"
    })).unwrap();
    let signer = DeviceProofSigner::new(&identity, "https://getbuild.ing",
        CanonicalOrigin::parse("https://bridge.example").unwrap(), "epoch-1".into()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = runtime_with(execution(), Arc::new(WitnessVerifier {
        calls: calls.clone(), authority: None,
    })).with_discovery(Arc::new(signer));
    (runtime, public, calls)
}

fn request(body: Body) -> Request<Body> {
    let mut request = Request::post("/.well-known/build-bridge")
        .header(CONTENT_TYPE, "application/json").body(body).unwrap();
    request.extensions_mut().insert(PeerAddress("192.0.2.1".parse().unwrap()));
    request
}

fn challenge(id: &str) -> Body {
    Body::from(json!({"challenge_id": id, "nonce": "n".repeat(43)}).to_string())
}

async fn response_json(response: Response, expected: StatusCode) -> serde_json::Value {
    assert_eq!(response.status(), expected);
    let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn discovery_post_serves_client_shape_and_verifiable_fixed_claims() {
    let (runtime, public, calls) = fixture();
    let mut req = request(challenge("challenge-1"));
    req.headers_mut().insert("forwarded", HeaderValue::from_static("host=attacker.example"));
    let body = response_json(runtime.router().oneshot(req).await.unwrap(), StatusCode::OK).await;
    assert_eq!(body.as_object().unwrap().len(), 1);
    let proof = body["proof"].as_str().unwrap();
    assert!(proof.len() <= 16000);
    let header = jsonwebtoken::decode_header(proof).unwrap();
    assert_eq!(header.alg, jsonwebtoken::Algorithm::EdDSA);
    assert_eq!(header.typ.as_deref(), Some("build-http-discovery+jwt"));
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::EdDSA);
    validation.set_audience(&["https://getbuild.ing/api/bridge-discovery/complete"]);
    let claims = jsonwebtoken::decode::<serde_json::Value>(proof,
        &jsonwebtoken::DecodingKey::from_ed_der(&public), &validation).unwrap().claims;
    assert_eq!(claims["challenge_id"], "challenge-1");
    assert_eq!(claims["nonce"], "n".repeat(43));
    assert_eq!(claims["origin"], "https://bridge.example");
    assert_eq!(claims["issuer"], "https://getbuild.ing");
    assert_eq!(claims["endpoint_epoch"], "epoch-1");
    assert_eq!(claims["iss"], "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    assert_eq!(claims["sub"], claims["iss"]);
    assert_eq!(claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(), 30);
    assert!(uuid::Uuid::parse_str(claims["jti"].as_str().unwrap()).is_ok());
    assert_eq!(claims.as_object().unwrap().len(), 11);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn discovery_refuses_malformed_oversized_and_replayed_bodies() {
    let (runtime, _, calls) = fixture();
    for body in ["{not-json".into(), "{}".into(),
        json!({"challenge_id":"x", "nonce":"short"}).to_string(),
        json!({"challenge_id":"x", "nonce":"n".repeat(129)}).to_string(),
        json!({"challenge_id":"x".repeat(129), "nonce":"n".repeat(43)}).to_string(),
        json!({"challenge_id":"x", "nonce":"n".repeat(43), "origin":"https://evil.example"}).to_string(),
    ] {
        let body = response_json(runtime.router().oneshot(request(Body::from(body))).await.unwrap(), StatusCode::BAD_REQUEST).await;
        assert_eq!(body["error_code"], "invalid_request");
        assert!(body.get("proof").is_none());
    }
    response_json(runtime.router().oneshot(request(Body::from("x".repeat(4097)))).await.unwrap(), StatusCode::PAYLOAD_TOO_LARGE).await;
    response_json(runtime.router().oneshot(request(challenge("once"))).await.unwrap(), StatusCode::OK).await;
    response_json(runtime.router().oneshot(request(challenge("once"))).await.unwrap(), StatusCode::CONFLICT).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn discovery_rate_limit_uses_socket_peer_and_shutdown_fences_signing() {
    let (runtime, _, _) = fixture();
    for index in 0..10 {
        let mut req = request(challenge(&format!("challenge-{index}")));
        req.headers_mut().insert("x-forwarded-for", HeaderValue::from_str(&format!("192.0.2.{index}")).unwrap());
        response_json(runtime.router().oneshot(req).await.unwrap(), StatusCode::OK).await;
    }
    response_json(runtime.router().oneshot(request(challenge("limited"))).await.unwrap(), StatusCode::TOO_MANY_REQUESTS).await;
    runtime.shutdown();
    response_json(runtime.router().oneshot(request(challenge("closed"))).await.unwrap(), StatusCode::SERVICE_UNAVAILABLE).await;
}
