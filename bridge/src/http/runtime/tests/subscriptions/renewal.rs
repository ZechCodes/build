//! Opening renewal: `PATCH /v1/sessions/{id}` carries a live stream past its
//! first grant, and nothing else does.
use super::*;

const GRANT: std::time::Duration = std::time::Duration::from_secs(30);
const OPENING_BEARER: &str = "account-bound-token";

type Outcome = Arc<Mutex<Result<&'static str, AuthError>>>;
/// Signals once a held renewal has minted its grant, then waits for release.
type Hold = Arc<
    Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
>;

/// Every verification mints a fresh `GRANT` over one shared token state, as
/// the production verifier's cache does. Renewal answers as told, and a
/// refusal of the opening's own token revokes that state, as hosted's
/// `active: false` does to the cached token.
struct MintingVerifier {
    token_live: Arc<AtomicBool>,
    renewal: Outcome,
    hold: Hold,
}

#[async_trait]
impl GrantVerifier for MintingVerifier {
    async fn verify(&self, _bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        Ok(authority_with(self.token_live.clone(), GRANT))
    }

    async fn revalidate(&self, bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        let mut authority = authority_with(self.token_live.clone(), GRANT);
        let held = self.hold.lock().unwrap().take();
        if let Some((minted, release)) = held {
            minted.send(()).unwrap();
            release.await.unwrap();
        }
        let outcome = *self.renewal.lock().unwrap();
        if outcome == Err(AuthError::InvalidCredential) && bearer == OPENING_BEARER {
            self.token_live.store(false, Ordering::SeqCst);
        }
        authority.identity.family_id = outcome?.into();
        Ok(authority)
    }
}

struct Renewable {
    app: Router,
    renewal: Outcome,
    hold: Hold,
    session_id: String,
    generation: String,
    subscription: String,
    frames: Frames,
}

async fn renewable() -> Renewable {
    let renewal = Arc::new(Mutex::new(Ok("family-1")));
    let hold = Arc::new(Mutex::new(None));
    let app = router_with(
        pushing_execution(),
        Arc::new(MintingVerifier {
            token_live: Arc::new(AtomicBool::new(true)),
            renewal: renewal.clone(),
            hold: hold.clone(),
        }),
    );
    let opening = opened(&app).await;
    let session_id = opening["session_id"].as_str().unwrap().to_owned();
    let subscription = subscription(&app, &session_id).await;
    let mut frames = stream(&app, &session_id, &subscription).await;
    assert_eq!(next_event(&mut frames).await.0, "hello");
    Renewable {
        app,
        renewal,
        hold,
        session_id,
        generation: opening["opening_generation"].as_str().unwrap().to_owned(),
        subscription,
        frames,
    }
}

async fn renew_as(
    app: Router,
    session_id: String,
    bearer: &str,
) -> (StatusCode, serde_json::Value) {
    let path = format!("/v1/sessions/{session_id}");
    let mut request = lifecycle("PATCH", &path, &session_id, None);
    request
        .headers_mut()
        .insert(AUTHORIZATION, format!("Bearer {bearer}").parse().unwrap());
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn expiry(opening: &serde_json::Value) -> time::OffsetDateTime {
    time::OffsetDateTime::parse(
        opening["grant_expires_at"].as_str().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
}

impl Renewable {
    async fn renew(&self) -> (StatusCode, serde_json::Value) {
        renew_as(self.app.clone(), self.session_id.clone(), OPENING_BEARER).await
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

    async fn assert_delivers(&mut self, why: &str) {
        assert_eq!(self.push().await, StatusCode::OK, "{why}");
        let frame = self.next_frame().await.unwrap_or_else(|| panic!("{why}"));
        assert!(frame.unwrap().starts_with("event: event\n"), "{why}");
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
    assert!(expiry(&renewed) > time::OffsetDateTime::now_utc() + time::Duration::seconds(25));

    tokio::time::advance(std::time::Duration::from_secs(15)).await;
    opening
        .assert_delivers("the push arrives after the first grant's deadline")
        .await;
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

    let (status, _) = renew_as(
        opening.app.clone(),
        opening.session_id.clone(),
        "junk-token",
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    opening
        .assert_delivers("an unverified request changes nothing")
        .await;
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
    opening
        .assert_delivers("open until the existing deadline")
        .await;
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
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    opening
        .assert_delivers("the 404 leaves the original stream untouched")
        .await;
    tokio::time::advance(std::time::Duration::from_secs(6)).await;
    opening.assert_ended().await;
}

#[tokio::test(start_paused = true)]
async fn an_older_renewal_landing_late_cannot_shorten_a_newer_one() {
    let mut opening = renewable().await;
    let (minted, minting) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    *opening.hold.lock().unwrap() = Some((minted, released));
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    let older = tokio::spawn(renew_as(
        opening.app.clone(),
        opening.session_id.clone(),
        OPENING_BEARER,
    ));
    tokio::time::timeout(std::time::Duration::from_secs(2), minting)
        .await
        .expect("the older renewal reaches hosted")
        .unwrap();
    tokio::time::advance(std::time::Duration::from_secs(10)).await;

    let (status, newer) = opening.renew().await;
    assert_eq!(status, StatusCode::OK, "{newer}");
    release.send(()).unwrap();
    let (status, older) = older.await.unwrap();

    assert_eq!(status, StatusCode::OK, "{older}");
    assert!(
        (expiry(&older) - expiry(&newer)).abs() < time::Duration::seconds(1),
        "the late reply reports the deadline actually in force: {} vs {}",
        older["grant_expires_at"],
        newer["grant_expires_at"]
    );
    tokio::time::advance(std::time::Duration::from_secs(25)).await;
    opening
        .assert_delivers("the newer renewal's deadline still holds")
        .await;
    tokio::time::advance(std::time::Duration::from_secs(6)).await;
    opening.assert_ended().await;
}

#[tokio::test(start_paused = true)]
async fn a_renewal_after_expiry_leaves_the_stream_ended() {
    let mut opening = renewable().await;
    tokio::time::advance(GRANT + std::time::Duration::from_secs(1)).await;
    opening.assert_ended().await;

    let (status, refused) = opening.renew().await;

    assert_eq!(status, StatusCode::UNAUTHORIZED, "{refused}");
    assert_eq!(opening.push().await, StatusCode::UNAUTHORIZED);
}
