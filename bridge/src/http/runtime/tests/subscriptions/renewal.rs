//! Opening renewal: `PATCH /v1/sessions/{id}` carries a live stream past its
//! first grant, and nothing else does.
use super::*;

const GRANT: std::time::Duration = std::time::Duration::from_secs(30);

/// Every verification mints a fresh `GRANT` over one shared token state, as
/// the production verifier's cache does; renewal answers as told.
struct MintingVerifier {
    token_live: Arc<AtomicBool>,
    renewal: Arc<Mutex<Result<&'static str, AuthError>>>,
}

#[async_trait]
impl GrantVerifier for MintingVerifier {
    async fn verify(&self, _bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        Ok(authority_with(self.token_live.clone(), GRANT))
    }

    async fn revalidate(&self, _bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        let family = (*self.renewal.lock().unwrap())?;
        let mut authority = authority_with(self.token_live.clone(), GRANT);
        authority.identity.family_id = family.into();
        Ok(authority)
    }
}

struct Renewable {
    app: Router,
    token_live: Arc<AtomicBool>,
    renewal: Arc<Mutex<Result<&'static str, AuthError>>>,
    session_id: String,
    generation: String,
    subscription: String,
    frames: Frames,
}

async fn renewable() -> Renewable {
    let token_live = Arc::new(AtomicBool::new(true));
    let renewal = Arc::new(Mutex::new(Ok("family-1")));
    let app = router_with(
        pushing_execution(),
        Arc::new(MintingVerifier {
            token_live: token_live.clone(),
            renewal: renewal.clone(),
        }),
    );
    let opening = opened(&app).await;
    let session_id = opening["session_id"].as_str().unwrap().to_owned();
    let subscription = subscription(&app, &session_id).await;
    let mut frames = stream(&app, &session_id, &subscription).await;
    assert_eq!(next_event(&mut frames).await.0, "hello");
    Renewable {
        app,
        token_live,
        renewal,
        session_id,
        generation: opening["opening_generation"].as_str().unwrap().to_owned(),
        subscription,
        frames,
    }
}

impl Renewable {
    async fn renew(&self) -> (StatusCode, serde_json::Value) {
        let path = format!("/v1/sessions/{}", self.session_id);
        let response = self
            .app
            .clone()
            .oneshot(lifecycle("PATCH", &path, &self.session_id, None))
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn push(&self) -> StatusCode {
        greeting_request(&self.app, &self.session_id, Some(&self.subscription))
            .await
            .0
    }

    /// The next frame that is not a heartbeat, or `None` once the stream ends.
    async fn next_frame(&mut self) -> Option<Result<String, ()>> {
        loop {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(2), self.frames.next())
                .await
                .expect("a frame or the end arrives promptly")?;
            let Ok(frame) = frame else {
                return Some(Err(()));
            };
            let text = String::from_utf8(frame.to_vec()).unwrap();
            if !text.starts_with(':') {
                return Some(Ok(text));
            }
        }
    }

    async fn assert_ended(&mut self) {
        let next = self.next_frame().await;
        assert!(
            matches!(next, None | Some(Err(()))),
            "no frame after authority ends: {next:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn renewing_the_opening_carries_its_stream_past_the_first_grant() {
    let mut opening = renewable().await;
    tokio::time::advance(std::time::Duration::from_secs(20)).await;

    let (status, renewed) = opening.renew().await;

    assert_eq!(status, StatusCode::OK, "{renewed}");
    assert_eq!(renewed["session_id"], opening.session_id.as_str());
    assert_eq!(renewed["opening_generation"], opening.generation.as_str());
    assert!(renewed["greeting"]["bridge_epoch"].is_string(), "{renewed}");
    let expires = time::OffsetDateTime::parse(
        renewed["grant_expires_at"].as_str().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert!(expires > time::OffsetDateTime::now_utc() + time::Duration::seconds(25));

    tokio::time::advance(std::time::Duration::from_secs(15)).await;
    assert_eq!(opening.push().await, StatusCode::OK);
    let pushed = opening
        .next_frame()
        .await
        .expect("the stream is still open");
    assert!(
        pushed.unwrap().starts_with("event: event\n"),
        "the push arrives after the first grant's deadline"
    );
}

#[tokio::test(start_paused = true)]
async fn an_unrenewed_opening_ends_its_stream_at_the_first_deadline() {
    let mut opening = renewable().await;

    tokio::time::advance(GRANT + std::time::Duration::from_secs(1)).await;

    opening.assert_ended().await;
}

#[tokio::test(start_paused = true)]
async fn hosted_revocation_seen_at_renewal_ends_the_stream_now() {
    let mut opening = renewable().await;
    // Hosted answering inactive revokes the token's shared state.
    opening.token_live.store(false, Ordering::SeqCst);
    *opening.renewal.lock().unwrap() = Err(AuthError::InvalidCredential);

    let (status, refused) = opening.renew().await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(refused["error_code"], "invalid_credential");
    opening.assert_ended().await;
}

#[tokio::test(start_paused = true)]
async fn a_refused_bearer_cannot_end_someone_elses_opening() {
    let mut opening = renewable().await;
    *opening.renewal.lock().unwrap() = Err(AuthError::InvalidCredential);

    let (status, _) = opening.renew().await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(opening.push().await, StatusCode::OK);
    assert!(opening
        .next_frame()
        .await
        .expect("an unverified request changes nothing")
        .unwrap()
        .starts_with("event: event\n"));
}

#[tokio::test(start_paused = true)]
async fn an_unavailable_renewal_leaves_only_the_existing_deadline() {
    let mut opening = renewable().await;
    *opening.renewal.lock().unwrap() = Err(AuthError::Unavailable);
    tokio::time::advance(std::time::Duration::from_secs(20)).await;

    let (status, refused) = opening.renew().await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refused["retryable"], true);
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    assert_eq!(opening.push().await, StatusCode::OK);
    assert!(opening
        .next_frame()
        .await
        .expect("open until the existing deadline")
        .unwrap()
        .starts_with("event: event\n"));
    tokio::time::advance(std::time::Duration::from_secs(6)).await;
    opening.assert_ended().await;
}

#[tokio::test(start_paused = true)]
async fn another_binding_cannot_renew_the_opening() {
    let mut opening = renewable().await;
    *opening.renewal.lock().unwrap() = Ok("family-2");
    tokio::time::advance(std::time::Duration::from_secs(20)).await;

    let (status, refused) = opening.renew().await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(refused["error_code"], "not_found");
    tokio::time::advance(std::time::Duration::from_secs(11)).await;
    opening.assert_ended().await;
}
