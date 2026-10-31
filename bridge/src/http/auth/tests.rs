use super::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct TestSigner {
    url: String,
    assertion_at: Mutex<Option<Instant>>,
}
impl AssertionSigner for TestSigner {
    fn issuer(&self) -> &str {
        "https://getbuild.ing"
    }
    fn device_id(&self) -> &str {
        "device-a"
    }
    fn origin(&self) -> &str {
        "https://bridge.example"
    }
    fn endpoint_epoch(&self) -> &str {
        "epoch-a"
    }
    fn introspection_url(&self) -> String {
        self.url.clone()
    }
    fn assertion(&self, hash: &str, _: u64) -> Result<String, AuthError> {
        *self.assertion_at.lock().unwrap() = Some(Instant::now());
        Ok(format!("assertion-{hash}"))
    }
}

fn test_verifier(server: &MockServer) -> HttpAuthVerifier {
    test_verifier_with_signer(server).0
}

fn test_verifier_with_signer(server: &MockServer) -> (HttpAuthVerifier, Arc<TestSigner>) {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap();
    let signer = Arc::new(TestSigner {
        url: format!("{}/api/bridge-http/introspect", server.uri()),
        assertion_at: Mutex::new(None),
    });
    (HttpAuthVerifier::configured(signer.clone(), client), signer)
}

fn valid_response() -> Value {
    json!({
        "active": true, "issuer": "https://getbuild.ing", "account_id": "account-a", "device_id": "device-a",
        "origin": "https://bridge.example", "endpoint_epoch": "epoch-a", "authorization_epoch": 1,
        "family_id": "family-a", "token_id": "token-id-a", "scope": "bridge:read bridge:write",
        "exp": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 300,
    })
}

async fn serve(server: &MockServer, body: Value) {
    Mock::given(method("POST"))
        .and(path("/api/bridge-http/introspect"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn only_a_complete_hosted_binding_can_mint_a_grant() {
    let server = MockServer::start().await;
    let (verifier, signer) = test_verifier_with_signer(&server);
    serve(&server, valid_response()).await;
    let grant = verifier.verify("device-token").await.unwrap();
    assert_eq!(grant.principal(), "device_user");
    assert_eq!(grant.account_id(), "account-a");
    assert_eq!(grant.binding().authorization_epoch(), 1);
    assert!(grant.check().is_ok());
    let assertion_at = signer.assertion_at.lock().unwrap().unwrap();
    assert!(grant.deadline() <= assertion_at + FRESHNESS);
    assert!(grant.refresh_at() <= assertion_at + REFRESH_AFTER);
    assert_eq!(grant.ensure_scope("bridge:read"), Ok(()));
    assert_eq!(grant.ensure_scope("admin"), Err(AuthError::ForbiddenScope));
}

#[tokio::test]
async fn every_target_binding_and_unknown_scope_is_rejected() {
    for (field, value) in [
        ("issuer", "https://dev.getbuild.ing"),
        ("device_id", "device-b"),
        ("origin", "https://evil.example"),
        ("endpoint_epoch", "epoch-old"),
        ("scope", "admin"),
    ] {
        let server = MockServer::start().await;
        let verifier = test_verifier(&server);
        let mut response = valid_response();
        response[field] = json!(value);
        serve(&server, response).await;
        assert_eq!(
            verifier.verify("device-token").await.err(),
            Some(AuthError::InvalidCredential),
            "{field}"
        );
    }
}

#[tokio::test]
async fn absent_inactive_and_malformed_hosted_responses_never_mint_authority() {
    for field in [
        "account_id",
        "family_id",
        "authorization_epoch",
        "scope",
        "exp",
        "token_id",
    ] {
        let server = MockServer::start().await;
        let verifier = test_verifier(&server);
        let mut response = valid_response();
        response.as_object_mut().unwrap().remove(field);
        serve(&server, response).await;
        assert_eq!(
            verifier.verify("device-token").await.err(),
            Some(AuthError::Unavailable),
            "{field}"
        );
    }
    let server = MockServer::start().await;
    serve(&server, json!({ "active": false })).await;
    assert_eq!(
        test_verifier(&server).verify("device-token").await.err(),
        Some(AuthError::InvalidCredential)
    );
}

#[tokio::test]
async fn requests_use_fixed_path_and_token_bound_assertions_without_caller_claims() {
    let server = MockServer::start().await;
    let token = "secret-device-token";
    let assertion = format!("assertion-{}", hex_digest(&token_digest(token).unwrap()));
    Mock::given(method("POST"))
        .and(path("/api/bridge-http/introspect"))
        .and(body_json(
            json!({"token": token, "client_assertion": assertion}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(valid_response()))
        .expect(1)
        .mount(&server)
        .await;
    test_verifier(&server).verify(token).await.unwrap();
}

#[tokio::test]
async fn cache_holds_only_digests_and_verification_is_deduplicated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(valid_response())
                .set_delay(Duration::from_millis(30)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let verifier = test_verifier(&server);
    let (first, second, third) = tokio::join!(
        verifier.verify("device-token"),
        verifier.verify("device-token"),
        verifier.verify("device-token")
    );
    first.unwrap();
    second.unwrap();
    third.unwrap();
    verifier.verify("device-token").await.unwrap();
    let cache = verifier.inner.cache.lock().unwrap();
    assert_eq!(cache.len(), 1);
    assert!(cache.contains_key(&token_digest("device-token").unwrap()));
}

#[tokio::test]
async fn expiry_caps_both_cached_admission_and_output() {
    let server = MockServer::start().await;
    let mut response = valid_response();
    let expiry = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 2;
    response["exp"] = json!(expiry);
    serve(&server, response).await;
    let grant = test_verifier(&server).verify("device-token").await.unwrap();
    assert!(grant.deadline() < Instant::now() + Duration::from_secs(2));
    tokio::time::sleep_until(grant.deadline()).await;
    assert_eq!(grant.check(), Err(AuthError::Expired));
    assert_eq!(grant.ensure_scope("bridge:write"), Err(AuthError::Closed));
}

#[tokio::test]
async fn response_delay_does_not_extend_freshness() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(valid_response())
                .set_delay(Duration::from_millis(70)),
        )
        .mount(&server)
        .await;
    let (verifier, signer) = test_verifier_with_signer(&server);
    let grant = verifier.verify("device-token").await.unwrap();
    let assertion_at = signer.assertion_at.lock().unwrap().unwrap();
    assert!(grant.deadline() <= assertion_at + FRESHNESS);
    assert!(grant.deadline() < Instant::now() + FRESHNESS - Duration::from_millis(40));
}

#[tokio::test]
async fn an_expired_response_cannot_mint_a_grant() {
    let server = MockServer::start().await;
    let mut response = valid_response();
    response["exp"] = json!(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs());
    serve(&server, response).await;
    assert_eq!(
        test_verifier(&server).verify("device-token").await.err(),
        Some(AuthError::InvalidCredential)
    );
}

#[tokio::test]
async fn redirect_never_forwards_a_token_or_assertion() {
    let target = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(valid_response()))
        .expect(0)
        .mount(&target)
        .await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(307).insert_header("Location", target.uri()))
        .mount(&server)
        .await;
    assert_eq!(
        test_verifier(&server).verify("device-token").await.err(),
        Some(AuthError::Unavailable)
    );
}

#[tokio::test]
async fn oversized_bodies_and_non_json_responses_are_unavailable() {
    for template in [
        ResponseTemplate::new(200).set_body_string("x".repeat(RESPONSE_LIMIT + 1)),
        ResponseTemplate::new(200).set_body_string("not json"),
        ResponseTemplate::new(500),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(template)
            .mount(&server)
            .await;
        assert_eq!(
            test_verifier(&server).verify("device-token").await.err(),
            Some(AuthError::Unavailable)
        );
    }
}

#[tokio::test]
async fn failed_refresh_does_not_extend_old_authority() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    serve(&server, valid_response()).await;
    let grant = verifier.verify("device-token").await.unwrap();
    let deadline = grant.deadline();
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    assert_eq!(
        verifier.refresh(&grant, "device-token").await.err(),
        Some(AuthError::Unavailable)
    );
    assert_eq!(grant.deadline(), deadline);
    assert!(grant.check().is_ok());
    assert_eq!(
        verifier.verify("uncached-token").await.err(),
        Some(AuthError::Unavailable)
    );
    assert!(verifier.verify("device-token").await.is_ok());
}

#[tokio::test]
async fn revocation_immediately_fences_all_grants_from_the_same_token() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    serve(&server, valid_response()).await;
    let grant = verifier.verify("device-token").await.unwrap();
    let second = verifier.verify("device-token").await.unwrap();
    server.reset().await;
    serve(&server, json!({"active": false})).await;
    assert_eq!(
        verifier.refresh(&grant, "device-token").await.err(),
        Some(AuthError::InvalidCredential)
    );
    assert_eq!(grant.check(), Err(AuthError::InvalidCredential));
    assert_eq!(second.check(), Err(AuthError::InvalidCredential));
}

#[tokio::test]
async fn rotation_preserves_principal_and_binding_but_closing_never_revives() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    serve(&server, valid_response()).await;
    let grant = verifier.verify("old-device-token").await.unwrap();
    let old_deadline = grant.deadline();
    tokio::time::sleep(Duration::from_millis(10)).await;
    server.reset().await;
    let mut rotated = valid_response();
    rotated["token_id"] = json!("new-token-id");
    rotated["scope"] = json!("bridge:write bridge:read");
    serve(&server, rotated).await;
    let refreshed = verifier
        .refresh(&grant, "rotated-device-token")
        .await
        .unwrap();
    assert!(refreshed.deadline() > old_deadline);
    assert_eq!(refreshed.principal(), grant.principal());
    assert_eq!(refreshed.binding(), grant.binding());
    grant.close();
    assert_eq!(refreshed.check(), Err(AuthError::Closed));
    assert_eq!(
        verifier.refresh(&grant, "rotated-device-token").await.err(),
        Some(AuthError::Closed)
    );
}

#[tokio::test]
async fn changed_immutable_binding_requires_reopening() {
    for (field, value) in [
        ("account_id", json!("account-b")),
        ("family_id", json!("family-b")),
        ("authorization_epoch", json!(2)),
        ("scope", json!("bridge:read")),
    ] {
        let server = MockServer::start().await;
        let verifier = test_verifier(&server);
        serve(&server, valid_response()).await;
        let grant = verifier.verify("old-device-token").await.unwrap();
        server.reset().await;
        let mut changed = valid_response();
        changed[field] = value;
        serve(&server, changed).await;
        assert_eq!(
            verifier.refresh(&grant, "rotated-token").await.err(),
            Some(AuthError::BindingMismatch)
        );
        assert_eq!(grant.check(), Err(AuthError::Closed));
    }
}

#[tokio::test]
async fn closing_while_introspection_is_in_flight_cannot_be_reversed() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    serve(&server, valid_response()).await;
    let grant = verifier.verify("device-token").await.unwrap();
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(valid_response())
                .set_delay(Duration::from_millis(50)),
        )
        .mount(&server)
        .await;
    let (result, ()) = tokio::join!(verifier.refresh(&grant, "device-token"), async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        grant.close();
    });
    assert_eq!(result.err(), Some(AuthError::Closed));
    assert_eq!(grant.check(), Err(AuthError::Closed));
}

#[tokio::test]
async fn exhausted_waiting_and_cache_capacity_refuse_without_a_network_request() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    let waiting = verifier
        .inner
        .waiters
        .acquire_many(WAITER_LIMIT as u32)
        .await
        .unwrap();
    assert_eq!(
        verifier.verify("device-token").await.err(),
        Some(AuthError::Overloaded)
    );
    drop(waiting);
    serve(&server, valid_response()).await;
    let grant = verifier.verify("device-token").await.unwrap();
    let evidence = grant.lease.evidence.lock().unwrap().clone();
    {
        let mut cache = verifier.inner.cache.lock().unwrap();
        for index in 0..CACHE_LIMIT - 1 {
            cache.insert(
                Sha256::digest(format!("token-{index}").as_bytes()).into(),
                CacheEntry {
                    state: evidence.token_state.clone(),
                    evidence: Some(evidence.clone()),
                    pending: None,
                },
            );
        }
    }
    assert_eq!(
        verifier.verify("new-device-token").await.err(),
        Some(AuthError::Overloaded)
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn hung_network_is_bounded_and_cached_grants_keep_their_original_deadline() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    serve(&server, valid_response()).await;
    let grant = verifier.verify("device-token").await.unwrap();
    let deadline = grant.deadline();
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(valid_response())
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&server)
        .await;
    let started = Instant::now();
    assert_eq!(
        verifier.refresh(&grant, "device-token").await.err(),
        Some(AuthError::Unavailable)
    );
    assert!(Instant::now() - started < Duration::from_millis(2500));
    assert_eq!(grant.deadline(), deadline);
}

#[tokio::test]
async fn invalid_bearers_are_refused_before_any_request() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    for token in ["", "a b", "a\nb", &"a".repeat(TOKEN_LIMIT + 1)] {
        assert_eq!(
            verifier.verify(token).await.err(),
            Some(AuthError::InvalidCredential)
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn production_destination_is_https_and_fixed_to_the_approved_issuer() {
    for url in [
        "http://getbuild.ing/api/bridge-http/introspect",
        "https://evil.example/api/bridge-http/introspect",
        "https://getbuild.ing/api/bridge-http/introspect?redirect=1",
        "https://getbuild.ing/api/bridge-http/introspect#fragment",
        "https://user@getbuild.ing/api/bridge-http/introspect",
    ] {
        assert_eq!(
            validate_destination(&TestSigner {
                url: url.into(),
                assertion_at: Mutex::new(None)
            }),
            Err(AuthError::InvalidConfiguration)
        );
    }
    assert_eq!(
        validate_destination(&TestSigner {
            url: "https://getbuild.ing/api/bridge-http/introspect".into(),
            assertion_at: Mutex::new(None)
        }),
        Ok(())
    );
}

#[tokio::test]
async fn cancelled_callers_cannot_expand_the_outstanding_lookup_queue() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(valid_response())
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&server)
        .await;
    // Poll each future once to launch its lookup, then abandon its receiver.
    for index in 0..WAITER_LIMIT {
        let token = format!("cancelled-token-{index}");
        let request = verifier.verify(&token);
        tokio::pin!(request);
        tokio::select! {
            biased;
            result = &mut request => panic!("unexpected early result: {:?}", result.err()),
            () = tokio::task::yield_now() => (),
        }
    }
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(20), verifier.verify("overflow-token"))
            .await
            .map(|result| result.err()),
        Ok(Some(AuthError::Overloaded))
    );
    assert_eq!(verifier.inner.cache.lock().unwrap().len(), WAITER_LIMIT);
}

#[tokio::test]
async fn verifier_futures_can_be_spawned_by_http_handlers() {
    fn assert_send<T: Send>(_: T) {}
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    assert_send(verifier.verify("device-token"));
}

#[tokio::test]
async fn independent_monotonic_deadline_fences_long_lived_tokens_and_old_cache_entries() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(valid_response()))
        .expect(2)
        .mount(&server)
        .await;
    let old = verifier.verify("device-token").await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(old.check(), Err(AuthError::Expired));
    tokio::time::resume();
    let fresh = verifier.verify("device-token").await.unwrap();
    assert!(fresh.check().is_ok());
    assert_eq!(old.check(), Err(AuthError::Closed));
    assert_eq!(
        verifier.refresh(&old, "device-token").await.err(),
        Some(AuthError::Closed)
    );
}

#[tokio::test]
async fn no_more_than_eight_real_network_lookups_can_be_in_flight() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    let (arrivals, mut observed) = watch::channel(0usize);
    let response = valid_response();
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            arrivals.send_modify(|value| *value += 1);
            ResponseTemplate::new(200)
                .set_body_json(response.clone())
                .set_delay(Duration::from_secs(1))
        })
        .mount(&server)
        .await;
    let mut calls = Vec::new();
    for index in 0..NETWORK_LIMIT + 4 {
        let verifier = verifier.clone();
        calls.push(tokio::spawn(async move {
            verifier.verify(&format!("network-token-{index}")).await
        }));
    }
    tokio::time::timeout(
        Duration::from_millis(900),
        observed.wait_for(|value| *value >= NETWORK_LIMIT),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(*observed.borrow(), NETWORK_LIMIT);
    assert_eq!(verifier.inner.network.available_permits(), 0);
    for call in calls {
        call.abort();
    }
}

#[tokio::test]
async fn local_disable_fences_evicted_grants_and_all_future_admissions() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    serve(&server, valid_response()).await;
    let grant = verifier.verify("device-token").await.unwrap();
    verifier.inner.cache.lock().unwrap().clear();
    verifier.close();
    assert_eq!(grant.check(), Err(AuthError::Closed));
    assert_eq!(
        verifier.verify("new-token").await.err(),
        Some(AuthError::Closed)
    );
}

#[tokio::test]
async fn local_disable_during_a_network_lookup_cannot_issue_a_late_grant() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    let (arrived, mut observed) = watch::channel(false);
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            arrived.send(true).unwrap();
            ResponseTemplate::new(200)
                .set_body_json(valid_response())
                .set_delay(Duration::from_millis(50))
        })
        .mount(&server)
        .await;
    let (result, ()) = tokio::join!(verifier.verify("device-token"), async {
        observed.wait_for(|value| *value).await.unwrap();
        verifier.close();
    });
    assert_eq!(result.err(), Some(AuthError::Closed));
}

#[tokio::test]
async fn refresh_cannot_transfer_a_live_lease_between_verifier_instances() {
    let server = MockServer::start().await;
    let original = test_verifier(&server);
    let replacement = test_verifier(&server);
    serve(&server, valid_response()).await;
    let grant = original.verify("device-token").await.unwrap();
    assert_eq!(
        replacement.refresh(&grant, "device-token").await.err(),
        Some(AuthError::BindingMismatch)
    );
    original.close();
    assert_eq!(grant.check(), Err(AuthError::Closed));
}

#[tokio::test]
async fn a_response_arriving_after_token_expiry_cannot_create_authority() {
    let server = MockServer::start().await;
    let verifier = test_verifier(&server);
    Mock::given(method("POST"))
        .respond_with(|_: &wiremock::Request| {
            let mut response = valid_response();
            response["exp"] = json!(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 1
            );
            ResponseTemplate::new(200)
                .set_body_json(response)
                .set_delay(Duration::from_millis(1100))
        })
        .mount(&server)
        .await;
    assert_eq!(
        verifier.verify("device-token").await.err(),
        Some(AuthError::Expired)
    );
}

#[tokio::test]
async fn revalidation_asks_hosted_again_and_moves_the_deadline() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(valid_response()))
        .expect(2)
        .mount(&server)
        .await;
    let verifier = test_verifier(&server);
    let first = verifier.verify("device-token").await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let renewed = verifier.revalidate("device-token").await.unwrap();

    assert!(renewed.deadline() > first.deadline());
    assert_eq!(renewed.binding(), first.binding());
}

#[tokio::test]
async fn revalidation_refused_by_hosted_revokes_grants_from_that_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(valid_response()))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    serve(&server, json!({"active": false})).await;
    let verifier = test_verifier(&server);
    let first = verifier.verify("device-token").await.unwrap();

    assert_eq!(
        verifier.revalidate("device-token").await.err(),
        Some(AuthError::InvalidCredential)
    );
    assert_eq!(first.check(), Err(AuthError::InvalidCredential));
}
