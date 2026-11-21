use super::*;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, HOST, ORIGIN};
use serde_json::Value;
use std::sync::{atomic::AtomicBool, OnceLock};
use tower::ServiceExt;

use crate::execution::{session::HttpIdentity, ExecutionService, FrameHandler};
use crate::http::{listener::OutputAuthority, policy::HttpPolicy};
use crate::timing::FrameClock;

fn fixture() -> &'static Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../../../fixtures/api/http/terminal-websocket-v1.json"
        ))
        .unwrap()
    })
}

fn string(value: &Value) -> &str {
    value.as_str().unwrap()
}

fn number(value: &Value) -> u64 {
    value.as_u64().unwrap()
}

fn cases(key: &str) -> &[Value] {
    fixture()["ticket"][key].as_array().unwrap()
}

fn origin() -> &'static str {
    string(&fixture()["endpoint"]["caller_origin"])
}

fn alternate_origin() -> &'static str {
    string(&fixture()["endpoint"]["allowed_caller_origins"][1])
}

fn ticket_path() -> &'static str {
    string(&fixture()["ticket"]["path"])
}

fn upgrade_path() -> &'static str {
    string(&fixture()["handshake"]["path"])
}

fn host() -> &'static str {
    string(&fixture()["handshake"]["request"]["headers"]["host"])
}

fn json_request(method: &str, path: &str, body: Value) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(HOST, host())
        .header(ORIGIN, origin())
        .header(CONTENT_TYPE, "application/json")
        .header(AUTHORIZATION, "Bearer writer")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.extensions_mut().insert(OutputAuthority::default());
    request
}

async fn json_response(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

struct Auth {
    revoked: Arc<AtomicBool>,
    proof_age: Duration,
    proof_ages: HashMap<&'static str, Duration>,
}

impl Auth {
    fn authority(&self, bearer: &str) -> VerifiedAuthority {
        let live = self.revoked.clone();
        let follows_revocation = bearer == "writer";
        let origin = string(&fixture()["endpoint"]["bridge_https_origin"]);
        let mut scopes: Vec<String> =
            serde_json::from_value(fixture()["ticket"]["required_scopes"].clone()).unwrap();
        if bearer == "reader" {
            scopes.retain(|scope| scope != "bridge:write");
        }
        VerifiedAuthority {
            identity: HttpIdentity {
                issuer: "https://issuer.test".into(),
                account_id: "account".into(),
                device_id: "device".into(),
                family_id: "family".into(),
                origin: origin.into(),
                endpoint_epoch: "endpoint".into(),
                authorization_epoch: 1,
            },
            deadline: Instant::now()
                + self
                    .proof_ages
                    .get(bearer)
                    .copied()
                    .unwrap_or(self.proof_age),
            live: Arc::new(move || !follows_revocation || !live.load(Ordering::Acquire)),
            scopes: scopes.into(),
            credential_digest: Sha256::digest(bearer.as_bytes()).into(),
        }
    }
}

#[async_trait::async_trait]
impl super::super::GrantVerifier for Auth {
    async fn verify(&self, bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        Ok(self.authority(bearer))
    }

    async fn revalidate(&self, bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        self.verify(bearer).await
    }
}

struct Harness {
    runtime: super::super::HttpRuntime,
    app: axum::Router,
    auth: Arc<Auth>,
    opening: Value,
}

impl Harness {
    async fn new(proof_age: Duration) -> Self {
        Self::with_proof_ages(proof_age, HashMap::new()).await
    }

    async fn with_proof_ages(
        proof_age: Duration,
        proof_ages: HashMap<&'static str, Duration>,
    ) -> Self {
        let endpoint = &fixture()["endpoint"];
        let policy = HttpPolicy::new(
            string(&endpoint["bridge_https_origin"]),
            &serde_json::from_value::<Vec<String>>(endpoint["allowed_caller_origins"].clone())
                .unwrap(),
        )
        .unwrap();
        let execution = ExecutionService::new(
            FrameHandler::new(FrameClock::new(), |_, _, _| {
                unreachable!("no terminal RPCs")
            })
            .with_greeting_reads(|epoch| json!({"bridge_epoch":epoch})),
        );
        let auth = Arc::new(Auth {
            revoked: Arc::default(),
            proof_age,
            proof_ages,
        });
        let mut runtime = super::super::runtime_with(execution, auth.clone());
        runtime.state.terminal_tickets = Some(Arc::default());
        let upgrades = axum::Router::new()
            .route(upgrade_path(), axum::routing::get(upgrade))
            .with_state(runtime.state.clone());
        let app = policy.apply(runtime.router().merge(upgrades));
        let mut request = json_request("POST", "/v1/sessions", json!({"api_range":">=3 <4"}));
        request
            .headers_mut()
            .insert(AUTHORIZATION, HeaderValue::from_static("Bearer opening"));
        let (status, opening) = json_response(app.clone().oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::CREATED);
        Self {
            runtime,
            app,
            auth,
            opening,
        }
    }

    fn state(&self) -> &HttpState {
        &self.runtime.state
    }

    fn session_id(&self) -> &str {
        string(&self.opening["session_id"])
    }

    fn session(&self) -> SessionLease {
        self.state().openings.lock().unwrap()[self.session_id()]
            .session
            .clone()
    }

    fn ticket_request(&self, body: Option<&Value>) -> Request<Body> {
        let default = json!({"opening_generation": self.opening["opening_generation"]});
        let mut request = json_request("POST", ticket_path(), body.cloned().unwrap_or(default));
        request
            .headers_mut()
            .insert("x-build-session", self.session_id().parse().unwrap());
        request
    }

    async fn issue(&self) -> String {
        let (status, body) = json_response(
            self.app
                .clone()
                .oneshot(self.ticket_request(None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        string(&body["ticket"]).to_owned()
    }

    async fn admit_with_pending_ticket(&self) -> (TerminalAdmission, String) {
        let live = self.redeem(&self.issue().await, origin()).ok().unwrap();
        let pending = self.issue().await;
        self.assert_registry_entries(1, 1);
        (live, pending)
    }

    fn registry(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.state()
            .terminal_tickets
            .as_ref()
            .unwrap()
            .registry
            .lock()
            .unwrap()
    }

    fn assert_registry_entries(&self, pending: usize, last: usize) {
        let registry = self.registry();
        assert_eq!(
            registry.pending.len(),
            pending,
            "pending tickets in registry"
        );
        assert_eq!(
            registry.last.len(),
            last,
            "terminal predecessors in registry"
        );
    }

    async fn patch(&self, credential: &str) {
        let mut request = json_request(
            "PATCH",
            &format!("/v1/sessions/{}", self.session_id()),
            json!({}),
        );
        request.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {credential}").parse().unwrap(),
        );
        assert_eq!(
            self.app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::OK
        );
    }

    fn redeem(&self, token: &str, caller: &str) -> HttpResult<TerminalAdmission> {
        let request = json_request("GET", upgrade_path(), Value::Null);
        let mut request = request;
        request
            .headers_mut()
            .insert(ORIGIN, caller.parse().unwrap());
        let policy = HttpPolicy::new(
            string(&fixture()["endpoint"]["bridge_https_origin"]),
            &[origin().into(), alternate_origin().into()],
        )
        .unwrap();
        request.extensions_mut().insert(policy);
        let origin = policy::terminal_origin(&request)?;
        redeem(self.state(), token, origin, Arc::new(Sink))
    }
}

struct Sink;
impl EventSink for Sink {
    fn try_send(&self, _event: EncodedEvent) -> Result<(), DeliveryError> {
        Ok(())
    }
}

#[derive(Clone)]
struct Race(Arc<tokio::sync::Barrier>);

async fn upgrade(State(state): State<HttpState>, request: Request<Body>) -> Response {
    let origin = policy::terminal_origin(&request).unwrap();
    let offer = request.headers()["sec-websocket-protocol"]
        .to_str()
        .unwrap();
    let token = offer.split_once("ticket.").unwrap().1;
    request.extensions().get::<Race>().unwrap().0.wait().await;
    match redeem(&state, token, origin, Arc::new(Sink)) {
        Ok(admission) => {
            let mut response = StatusCode::SWITCHING_PROTOCOLS.into_response();
            response.extensions_mut().insert(Arc::new(admission));
            response
        }
        Err(response) => *response,
    }
}

fn upgrade_request(token: &str, race: Race) -> Request<Body> {
    let mut request = json_request("GET", upgrade_path(), Value::Null);
    request.headers_mut().remove(AUTHORIZATION);
    let headers = fixture()["handshake"]["request"]["headers"]
        .as_object()
        .unwrap();
    for (name, value) in headers {
        let value = string(value).replace(
            string(&fixture()["ticket"]["response"]["body"]["ticket"]),
            token,
        );
        request.headers_mut().insert(
            name.parse::<axum::http::HeaderName>().unwrap(),
            value.parse().unwrap(),
        );
    }
    request.extensions_mut().insert(race);
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_ticket_simultaneous_upgrades_admit_exactly_one_socket() {
    let harness = Harness::new(Duration::from_secs(30)).await;
    let token = harness.issue().await;
    let race = Race(Arc::new(tokio::sync::Barrier::new(2)));
    let first = tokio::spawn(
        harness
            .app
            .clone()
            .oneshot(upgrade_request(&token, race.clone())),
    );
    let second = tokio::spawn(harness.app.clone().oneshot(upgrade_request(&token, race)));
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    let (winner, loser) = if first.status() == StatusCode::SWITCHING_PROTOCOLS {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(
        u64::from(winner.status().as_u16()),
        number(&fixture()["handshake"]["response"]["status"])
    );
    assert!(winner
        .extensions()
        .get::<Arc<TerminalAdmission>>()
        .unwrap()
        .is_live());
    assert_fixture_refusal(
        loser,
        &fixture()["ticket"]["refusals"]["invalid_credential"],
    )
    .await;
    drop(winner);
    assert_eq!(
        harness.redeem(&token, origin()).err().unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(harness.redeem(&harness.issue().await, origin()).is_ok());
}

#[test]
fn terminal_ticket_decoder_accepts_only_canonical_fixture_tokens() {
    let handshake = &fixture()["handshake"];
    for offer in handshake["offers"].as_array().unwrap() {
        let status = number(&offer["status"]);
        if status == 400 {
            continue;
        }
        let token = offer["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|part| string(part).strip_prefix("ticket."));
        assert_eq!(
            token.and_then(digest_token).is_some(),
            status == 101,
            "{}",
            offer["name"]
        );
    }
}

#[test]
fn terminal_ticket_held_socket_cap_matches_fixture() {
    for case in fixture()["limits"]["socket_cases"].as_array().unwrap() {
        let held = |key: &str| case[key].as_u64().unwrap_or_default();
        let family = held("held_family") + held("superseded_held_family");
        let listener = held("held_listener") + held("superseded_held_listener");
        let tickets = TerminalTickets::default();
        let _family: Vec<_> = (0..family)
            .map(|_| tickets.reserve_socket("current-family").unwrap())
            .collect();
        let _others: Vec<_> = (family..listener)
            .map(|number| tickets.reserve_socket(&format!("family-{number}")).unwrap())
            .collect();
        assert_eq!(
            tickets.reserve_socket("current-family").is_ok(),
            case["admit"].as_bool().unwrap(),
            "{}",
            case["name"],
        );
    }
}

#[tokio::test]
async fn terminal_ticket_default_gate_and_shutdown_refuse_old_tickets() {
    let mut harness = Harness::new(Duration::from_secs(30)).await;
    let (live, pending) = harness.admit_with_pending_ticket().await;
    assert_eq!(harness.runtime.shutdown(), 1);
    assert!(!live.is_live());
    harness.assert_registry_entries(0, 0);
    assert!(harness.registry().full_at.is_empty());
    assert_eq!(
        harness.redeem(&pending, origin()).err().unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    harness.runtime.state.terminal_tickets = None;
    let response = harness
        .runtime
        .router()
        .oneshot(harness.ticket_request(None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(start_paused = true)]
async fn terminal_ticket_issue_rates_and_pending_caps_match_fixture() {
    let harness = Harness::new(Duration::from_secs(30)).await;
    for case in cases("issue_cases") {
        assert_issue_case(&harness, case);
    }
}

fn assert_issue_case(harness: &Harness, case: &Value) {
    let now = Instant::now();
    let mut registry = Registry::default();
    let authority = harness.auth.authority("writer");
    let target = PendingTicket::new(
        harness.session(),
        string(&harness.opening["opening_generation"]).into(),
        authority.clone(),
        origin().into(),
        now,
    );
    let seeded = seed_pending(harness, case, &mut registry, &target, now);
    let family = &authority.identity.family_id;
    for _ in 0..number(&case["prior_issues"]) {
        registry.charge_issue(family, now).unwrap();
    }
    let elapsed = Duration::from_millis(number(&case["elapsed_ms"]));
    let result = registry.issue(target, now + elapsed);
    let status = result
        .as_ref()
        .map_or_else(|response| response.status(), |_| StatusCode::CREATED);
    assert_eq!(
        u64::from(status.as_u16()),
        number(&case["status"]),
        "{}",
        case["name"]
    );
    let replaced = seeded.is_some_and(|digest| !registry.pending.contains_key(&digest));
    assert_eq!(
        replaced,
        case["superseded_previous"].as_bool().unwrap(),
        "{}",
        case["name"]
    );
}

fn seed_pending(
    harness: &Harness,
    case: &Value,
    registry: &mut Registry,
    target: &PendingTicket,
    now: Instant,
) -> Option<TicketDigest> {
    let family = number(&case["pending_family"]);
    let in_opening = number(&case["pending_opening"]);
    let mut seeded = None;
    for index in 0..number(&case["pending_global"]) {
        let digest = Sha256::digest(index.to_be_bytes()).into();
        let mut ticket = target.clone();
        if index < in_opening {
            seeded = Some(digest);
        } else {
            if index >= family {
                ticket.authority.identity.family_id = format!("other-{}", index / family);
            }
            ticket.session = harness
                .state()
                .execution
                .sessions()
                .open(ticket.authority.caller())
                .unwrap();
            ticket.expires = ticket_deadline(ticket.authority.deadline, now);
        }
        registry.pending.insert(digest, ticket);
    }
    seeded
}

#[tokio::test]
async fn terminal_ticket_supersedes_pending_and_rechecks_socket_capacity() {
    let harness = Harness::new(Duration::from_secs(30)).await;
    let first = harness.issue().await;
    let second = harness.issue().await;
    assert!(second != first);
    assert_eq!(
        harness.redeem(&first, origin()).err().unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let tickets = harness.state().terminal_tickets.as_ref().unwrap();
    let family = &harness.auth.authority("writer").identity.family_id;
    let limit = number(&fixture()["limits"]["sockets"]["held_family"]);
    let mut held: Vec<_> = (0..limit)
        .map(|_| tickets.reserve_socket(family).unwrap())
        .collect();
    assert_eq!(
        harness.redeem(&second, origin()).err().unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(held.pop());
    assert!(harness.redeem(&second, origin()).is_ok());
}

async fn assert_fixture_refusal(response: Response, expected: &Value) {
    let (status, mut body) = json_response(response).await;
    assert_eq!(u64::from(status.as_u16()), number(&expected["status"]));
    if expected["body"]["request_id"].is_string() {
        uuid::Uuid::parse_str(string(&body["request_id"])).unwrap();
        body["request_id"] = expected["body"]["request_id"].clone();
    }
    assert_eq!(body, expected["body"]);
}

#[tokio::test]
async fn terminal_ticket_post_matches_origin_vectors() {
    let harness = Harness::new(Duration::from_secs(30)).await;
    for case in cases("origin_cases") {
        let mut request = harness.ticket_request(None);
        request.headers_mut().remove(ORIGIN);
        for value in case["values"].as_array().unwrap() {
            request
                .headers_mut()
                .append(ORIGIN, string(value).parse().unwrap());
        }
        let response = harness.app.clone().oneshot(request).await.unwrap();
        if let Some(refusal) = case["refusal"].as_str() {
            assert_fixture_refusal(response, &fixture()["endpoint"]["policy_refusals"][refusal])
                .await;
        } else {
            assert_issue_response(&harness, response, case).await;
        }
    }
}

async fn assert_issue_response(harness: &Harness, response: Response, case: &Value) {
    let expected = &fixture()["ticket"]["response"];
    for (name, value) in expected["headers"].as_object().unwrap() {
        assert_eq!(response.headers()[name.as_str()], string(value));
    }
    let (status, body) = json_response(response).await;
    assert_eq!(u64::from(status.as_u16()), number(&case["status"]));
    let raw = URL_SAFE_NO_PAD.decode(string(&body["ticket"])).unwrap();
    assert_eq!(
        raw.len() as u64,
        number(&fixture()["ticket"]["token_bytes"])
    );
    assert_eq!(URL_SAFE_NO_PAD.encode(raw), string(&body["ticket"]));
    time::OffsetDateTime::parse(
        string(&body["expires_at"]),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert_eq!(
        body["opening_generation"],
        harness.opening["opening_generation"]
    );
    let token = string(&body["ticket"]);
    let bound = string(&case["bound_origin"]);
    assert_eq!(
        harness
            .redeem(
                token,
                if bound == origin() {
                    alternate_origin()
                } else {
                    origin()
                }
            )
            .err()
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(harness.redeem(token, bound).is_ok());
}

fn invalid_request(harness: &Harness, case: &Value) -> Request<Body> {
    let mut request = harness.ticket_request(case.get("body"));
    if let Some(header) = case["header"].as_str() {
        let header: axum::http::HeaderName = header.parse().unwrap();
        request.headers_mut().remove(&header);
        for value in case["values"].as_array().unwrap() {
            request
                .headers_mut()
                .append(&header, string(value).parse().unwrap());
        }
    }
    if case.get("verified_scopes").is_some() {
        request
            .headers_mut()
            .insert(AUTHORIZATION, "Bearer reader".parse().unwrap());
    }
    request
}

#[tokio::test]
async fn terminal_ticket_post_refuses_invalid_requests() {
    let harness = Harness::new(Duration::from_secs(30)).await;
    for case in cases("invalid_requests") {
        let held = case["held_family_sockets"].as_u64().unwrap_or_default();
        let registry = harness.state().terminal_tickets.as_ref().unwrap();
        let family = &harness.auth.authority("writer").identity.family_id;
        let _held: Vec<_> = (0..held)
            .map(|_| registry.reserve_socket(family).unwrap())
            .collect();
        let response = harness
            .app
            .clone()
            .oneshot(invalid_request(&harness, case))
            .await
            .unwrap();
        let code = string(&case["refusal"]);
        let spec = fixture()["ticket"]["refusals"]
            .get(code)
            .unwrap_or(&fixture()["endpoint"]["policy_refusals"][code]);
        assert_fixture_refusal(response, spec).await;
    }
}

#[tokio::test(start_paused = true)]
async fn terminal_ticket_expires_at_fixture_deadline() {
    for case in cases("lifetime_cases") {
        let parse = |key: &str| {
            time::OffsetDateTime::parse(
                string(&case[key]),
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap()
        };
        let harness = Harness::new(
            (parse("proof_deadline") - parse("issued_at"))
                .try_into()
                .unwrap(),
        )
        .await;
        let ticket = harness.issue().await;
        let delay: Duration = (parse("expires_at") - parse("issued_at"))
            .try_into()
            .unwrap();
        tokio::time::advance(delay).await;
        assert_eq!(
            harness.redeem(&ticket, origin()).err().unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        harness.state().reap_ended();
        assert!(harness
            .state()
            .terminal_tickets
            .as_ref()
            .unwrap()
            .registry
            .lock()
            .unwrap()
            .pending
            .is_empty());
    }
}

#[tokio::test]
async fn terminal_ticket_predecessor_is_compared_after_close() {
    let harness = Harness::new(Duration::from_secs(30)).await;
    let first = harness
        .redeem(&harness.issue().await, origin())
        .ok()
        .unwrap();
    let against_first = harness.issue().await;
    drop(first);
    let second = harness.redeem(&against_first, origin()).ok().unwrap();
    let against_second = harness.issue().await;
    let third = harness
        .state()
        .execution
        .sessions()
        .replace_events(
            second.session(),
            StreamClass::Terminal,
            Some(second.binding()),
            Arc::new(Sink),
        )
        .unwrap();
    assert_eq!(
        harness
            .redeem(&against_second, origin())
            .err()
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    drop(second);
    assert!(third.is_live());
}

#[tokio::test(start_paused = true)]
async fn terminal_ticket_exact_renewal_does_not_extend_captured_evidence() {
    let harness = Harness::new(Duration::from_secs(30)).await;
    let live = harness
        .redeem(&harness.issue().await, origin())
        .ok()
        .unwrap();
    let (_, captured) = harness
        .state()
        .execution
        .sessions()
        .resume_with_request_authority(live.capture_caller().unwrap(), live.session().id())
        .unwrap();
    let original = live.deadline();
    tokio::time::advance(Duration::from_secs(1)).await;
    harness.issue().await;
    assert_eq!(live.deadline(), original);
    harness.patch("writer").await;
    assert!(live.deadline() > original);
    assert_eq!(captured.deadline(), Some(original));
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(live.is_live());
    assert!(!captured.is_valid());
}

#[tokio::test]
async fn terminal_ticket_patch_rotation_or_scope_change_fences_authority() {
    for bearer in ["rotated", "reader"] {
        let harness = Harness::new(Duration::from_secs(30)).await;
        let live = harness
            .redeem(&harness.issue().await, origin())
            .ok()
            .unwrap();
        let ticket = harness.issue().await;
        harness.patch(bearer).await;
        assert!(!live.is_live(), "{bearer}");
        assert_eq!(
            harness.redeem(&ticket, origin()).err().unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(harness.session().authorization().is_valid());
    }
}

#[tokio::test]
async fn terminal_ticket_observed_revocation_fences_pending_and_live() {
    let harness = Harness::new(Duration::from_secs(30)).await;
    let live = harness
        .redeem(&harness.issue().await, origin())
        .ok()
        .unwrap();
    let ticket = harness.issue().await;
    harness.auth.revoked.store(true, Ordering::Release);
    assert!(harness.session().authorization().is_valid());
    assert!(!live.is_live());
    assert_eq!(
        harness.redeem(&ticket, origin()).err().unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test(start_paused = true)]
async fn terminal_ticket_live_admission_expires_before_its_opening() {
    let harness = Harness::with_proof_ages(
        Duration::from_secs(30),
        HashMap::from([("writer", Duration::from_secs(4))]),
    )
    .await;
    let live = harness
        .redeem(&harness.issue().await, origin())
        .ok()
        .unwrap();
    let lifecycle = harness.session().authorization();
    let proof_deadline = live.deadline();
    assert!(proof_deadline < lifecycle.deadline().unwrap());
    assert!(live.is_live());
    tokio::time::advance(proof_deadline.duration_since(Instant::now())).await;
    assert!(lifecycle.is_valid());
    assert!(live.binding().is_current());
    assert!(
        !live.is_live(),
        "terminal authority must end at its own proof deadline"
    );
}

#[tokio::test]
async fn terminal_ticket_fully_retired_opening_requires_a_new_opening() {
    let harness = Harness::new(Duration::from_secs(30)).await;
    let (live, ticket) = harness.admit_with_pending_ticket().await;
    let path = format!("/v1/sessions/{}", harness.session_id());
    let request = json_request("DELETE", &path, Value::Null);
    assert_eq!(
        harness.app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    harness.assert_registry_entries(0, 0);
    assert!(!live.is_live());
    assert_eq!(
        harness.redeem(&ticket, origin()).err().unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let response = harness
        .app
        .clone()
        .oneshot(harness.ticket_request(None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(start_paused = true)]
async fn terminal_ticket_reaped_opening_releases_registry_entries() {
    let opening_age = Duration::from_secs(30);
    let harness = Harness::new(opening_age).await;
    let (live, _pending) = harness.admit_with_pending_ticket().await;
    tokio::time::advance(opening_age).await;
    assert_eq!(harness.state().reap_ended(), 1);
    harness.assert_registry_entries(0, 0);
    assert!(!live.is_live());
}
