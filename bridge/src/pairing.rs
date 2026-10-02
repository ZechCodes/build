//! Device pairing — registering this bridge to a user account, then waiting for the
//! human to approve it.
//!
//! Flow (device-initiated, human-approved):
//! 1. The bridge generates a short, high-entropy **pairing code** and registers as
//!    *pending* with the api: it sends both public keys + a hash of the code +
//!    an Ed25519 signature over a challenge binding all of them (proof it holds the
//!    identity private key). The raw code is **never** sent — only its hash.
//! 2. The bridge prints the code + its key fingerprint and polls for approval.
//! 3. The user opens the printed approve link (or enters the code in the web
//!    app), compares the fingerprint out-of-band, and approves — which binds the
//!    device to their account. The link carries the code in its fragment and
//!    only fills it in: approving is still the user's press.
//! 4. The bridge sees `approved`, persists it, and connects to the relay.
//!
//! Pure request/response *shaping* lives in free functions (unit-tested without a
//! network); the HTTP calls are thin wrappers around an injected [`reqwest::Client`]
//! so they can be pointed at a mock server in tests.
//!
//! TLS: reqwest is built with its `rustls` feature (see `Cargo.toml`), so
//! `https://getbuild.ing` pairing calls use rustls with the platform trust store
//! via `rustls-platform-verifier` — never native-tls/OpenSSL. (reqwest 0.13
//! dropped the `rustls-tls-webpki-roots` option; the platform verifier is its
//! rustls root story.)

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::identity::{self, StoredIdentity};
use crate::transport;

/// Characters used in pairing codes — unambiguous (no 0/O, 1/I/L).
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// Pairing code length (excluding the separating dash); ~8 chars ≈ 40 bits.
const CODE_LEN: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("http error: {0}")]
    Http(String),
    #[error("api rejected registration: {0}")]
    Rejected(String),
    #[error("identity error: {0}")]
    Identity(String),
    /// The api said "not approved", but it is not known to be the api that
    /// approved this identity, so its word is not enough to retire it (#320).
    #[error("{asked} said not approved, but it is not known to have approved this device")]
    ApprovedElsewhere {
        /// The api that was asked.
        asked: String,
        /// The api the identity records as its approver, if it records one.
        approver: Option<String>,
    },
    /// While its code was shown, the api said this registration is gone, a
    /// few times in a row (#321 review).
    #[error("the api {0}, so this code can no longer be approved; run build-bridge pair again for a new one")]
    PairingEnded(&'static str),
}

type Result<T> = std::result::Result<T, PairingError>;

/// The registration payload POSTed to `/api/devices/register`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub device_id: String,
    pub name: String,
    pub identity_public_key_b64: String,
    pub transport_public_key_b64: String,
    /// SHA-256 hex of the pairing code — never the raw code.
    pub pairing_code_hash: String,
    /// Ed25519 signature (padded b64) over [`registration_challenge`], proving
    /// possession of the identity private key.
    pub signature_b64: String,
}

/// The approval-status response from `/api/devices/{id}/status`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StatusResponse {
    pub approved: bool,
    #[serde(default)]
    pub owner_user_id: Option<String>,
    /// Where the device stands, said by an api new enough to say (#317). An
    /// older api sends only `approved`, so this is optional.
    #[serde(default)]
    pub state: Option<RegistrationState>,
}

/// The api's `state`. Any state this bridge does not know reads as
/// [`RegistrationState::Other`], never as a parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistrationState {
    Approved,
    Pending,
    Revoked,
    Unknown,
    #[serde(other)]
    Other,
}

/// Why an approval this machine stored is no longer one the api honours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lapse {
    /// Revoked in Settings → Devices.
    Revoked,
    /// The api has no such device: never registered there, or purged since.
    Unknown,
    /// Not approved, and the api did not say why (it predates `state`).
    NotApproved,
}

impl std::fmt::Display for Lapse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Revoked => "it was revoked in Settings → Devices",
            Self::Unknown => "your account no longer has this device",
            Self::NotApproved => "the api says it is no longer approved",
        })
    }
}

impl StatusResponse {
    /// How an approval stored for this device has lapsed, if the api no
    /// longer approves it. Only meaningful for a device stored as approved: a
    /// pending one has nothing to lapse.
    pub fn lapse(&self) -> Option<Lapse> {
        if self.approved {
            return None;
        }
        Some(match self.state {
            Some(RegistrationState::Revoked) => Lapse::Revoked,
            Some(RegistrationState::Unknown) => Lapse::Unknown,
            _ => Lapse::NotApproved,
        })
    }
}

/// A stored approval `pair` moved aside because the api no longer honours it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetiredApproval {
    pub lapse: Lapse,
    /// The api that said "not approved" — named, because a wrong
    /// `BRIDGE_API_URL` is the one way a good identity gets retired.
    pub api_url: String,
    /// Where the identity lived, and where the next pairing writes a new one.
    pub identity_path: std::path::PathBuf,
    /// Where the old identity now lives.
    pub kept_at: std::path::PathBuf,
}

impl RetiredApproval {
    /// What `pair` says before it pairs again, indented under the installer's
    /// step and short enough for an 80-column terminal (#319). Where the old
    /// identity went is named by its directory, `~`-shortened against `home`;
    /// the file name and the way back are the README's to give. An api other
    /// than the default is named, because a wrong `BRIDGE_API_URL` is the one
    /// way a good identity gets retired.
    pub fn notice(&self, home: &Path) -> String {
        let kept_in = self.kept_at.parent().unwrap_or(Path::new("/"));
        let mut lines = vec![
            "    This machine's earlier pairing is no longer valid.".to_string(),
            wrapped(
                &format!(
                    "Pairing it as a new device; the old identity is kept in {}.",
                    home_shortened(kept_in, home)
                ),
                INDENT,
            ),
        ];
        if !same_api(&self.api_url, crate::config::DEFAULT_API_URL) {
            lines.push(wrapped(&format!("Asked {}", self.api_url), INDENT));
        }
        lines.join("\n")
    }
}

/// How far `pair` indents its lines: under the installer's `==>` step.
pub const INDENT: &str = "    ";

/// The widest line `pair` prints.
const COLUMNS: usize = 80;

/// `text` broken between words into lines of at most 80 columns, each
/// starting with `indent`. A word too long for a line gets one to itself.
pub fn wrapped(text: &str, indent: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::from(indent);
    for word in text.split_whitespace() {
        let fits = line.chars().count() + 1 + word.chars().count() <= COLUMNS;
        if line.len() > indent.len() && !fits {
            lines.push(std::mem::replace(&mut line, String::from(indent)));
        }
        if line.len() > indent.len() {
            line.push(' ');
        }
        line.push_str(word);
    }
    lines.push(line);
    lines.join("\n")
}

/// `path` as a person would type it: under `home` it starts with `~`.
fn home_shortened(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The page that approves the device whose code is `code`: the SPA reads the
/// code from the fragment, which a browser never sends to a server, and opens
/// the approve screen with it filled in. The person still confirms there.
pub fn approve_url(web_url: &str, code: &str) -> String {
    format!("{}/app/#/pair/{code}", web_url.trim_end_matches('/'))
}

/// The fingerprint as the approve screen shows it to compare: its first 32
/// hex digits (128 bits), in fours.
pub fn short_fingerprint(fingerprint: &str) -> String {
    let digits: Vec<char> = fingerprint.chars().take(32).collect();
    digits
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The block `pair` prints while it waits, indented under the installer's
/// step: one link that opens the approve screen with this device pulled up,
/// and the code and fingerprint to compare there. A link too long to follow
/// its label gets a line of its own, whole.
pub fn pairing_prompt(code: &str, fingerprint: &str, web_url: &str) -> String {
    format!(
        "\n{INDENT}pairing code:  {code}\n{INDENT}fingerprint:   {}\n{}\n\n{INDENT}\
         Waiting for you to approve it in Build…",
        short_fingerprint(fingerprint),
        approve_line(&approve_url(web_url, code))
    )
}

fn approve_line(url: &str) -> String {
    let inline = format!("{INDENT}approve at:    {url}");
    if inline.chars().count() <= COLUMNS {
        inline
    } else {
        format!("{INDENT}approve at:\n{INDENT}  {url}")
    }
}

// --- pure shaping -------------------------------------------------------------

/// Generate a high-entropy pairing code like `WXYZ-4F2K`, drawing from an
/// unambiguous alphabet. Uses the transport RNG.
pub fn generate_pairing_code() -> String {
    let bytes = transport::random_bytes(CODE_LEN);
    let mut code = String::with_capacity(CODE_LEN + 1);
    for (i, b) in bytes.iter().enumerate() {
        if i == CODE_LEN / 2 {
            code.push('-');
        }
        code.push(CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char);
    }
    code
}

/// SHA-256 hex of the pairing code. The api stores this and compares the hash of the
/// code the human types, so the raw code never leaves the operator's hands until then.
pub fn hash_pairing_code(code: &str) -> String {
    hex(&Sha256::digest(code.as_bytes()))
}

/// The canonical message signed during registration. Binds the device id, both public
/// keys, and the code hash so a captured signature cannot be replayed onto a different
/// registration. Pure string builder.
pub fn registration_challenge(
    device_id: &str,
    identity_public_key_b64: &str,
    transport_public_key_b64: &str,
    pairing_code_hash: &str,
) -> String {
    format!(
        "register.{device_id}.{identity_public_key_b64}.{transport_public_key_b64}.{pairing_code_hash}"
    )
}

/// Build a fully-signed [`RegisterRequest`] from a stored identity and a raw pairing
/// code. Signs [`registration_challenge`] with the identity private key. Does not
/// mutate its inputs.
pub fn build_register_request(
    identity: &StoredIdentity,
    pairing_code: &str,
) -> Result<RegisterRequest> {
    let pairing_code_hash = hash_pairing_code(pairing_code);
    let challenge = registration_challenge(
        &identity.device_id,
        &identity.identity_public_key_b64,
        &identity.transport.public_key_b64,
        &pairing_code_hash,
    );
    let signature_b64 =
        transport::sign_message_b64(&identity.identity_private_key_b64, challenge.as_bytes())
            .map_err(|e| PairingError::Identity(e.to_string()))?;
    Ok(RegisterRequest {
        device_id: identity.device_id.clone(),
        name: identity.name.clone(),
        identity_public_key_b64: identity.identity_public_key_b64.clone(),
        transport_public_key_b64: identity.transport.public_key_b64.clone(),
        pairing_code_hash,
        signature_b64,
    })
}

// --- I/O ----------------------------------------------------------------------

/// POST `{api}/api/devices/register`. 2xx → Ok; any other status → Rejected.
pub async fn register(
    client: &reqwest::Client,
    api_url: &str,
    req: &RegisterRequest,
) -> Result<()> {
    let url = format!("{}/api/devices/register", api_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(req)
        .send()
        .await
        .map_err(|e| PairingError::Http(e.to_string()))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(PairingError::Rejected(format!("{status}: {body}")))
    }
}

/// How long the status client waits to connect to the api.
pub const STATUS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one status call may take in all, connect included.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(30);

/// The client `pair` and the install gate ask `/status` with. Bounded, so a
/// hung api ends in an error (which retires nothing) instead of hanging the
/// installer.
pub fn status_client() -> reqwest::Client {
    status_client_with(STATUS_CONNECT_TIMEOUT, STATUS_TIMEOUT)
}

/// [`status_client`] with the bounds given — tests use short ones.
pub fn status_client_with(connect: Duration, total: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect)
        .timeout(total)
        .build()
        .expect("a client with only timeouts set always builds")
}

/// GET `{api}/api/devices/{device_id}/status` once.
pub async fn fetch_status(
    client: &reqwest::Client,
    api_url: &str,
    device_id: &str,
) -> Result<StatusResponse> {
    ask_status(client, api_url, device_id)
        .await
        .map_err(|(error, _)| error)
}

/// [`fetch_status`], saying of a status call that got no answer whether
/// asking again could get one.
async fn ask_status(
    client: &reqwest::Client,
    api_url: &str,
    device_id: &str,
) -> std::result::Result<StatusResponse, (PairingError, Unanswered)> {
    let unreachable =
        |e: reqwest::Error| (PairingError::Http(e.to_string()), Unanswered::Unreachable);
    let url = format!(
        "{}/api/devices/{device_id}/status",
        api_url.trim_end_matches('/')
    );
    let resp = client.get(&url).send().await.map_err(unreachable)?;
    let status = resp.status();
    if !status.is_success() {
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| crate::presence::retry_after(value, std::time::SystemTime::now()));
        return Err((
            PairingError::Rejected(status.to_string()),
            Unanswered::from_status(status, retry_after),
        ));
    }
    resp.json::<StatusResponse>().await.map_err(unreachable)
}

/// Why a status call got no answer, by whether asking again could get one.
#[derive(Debug)]
enum Unanswered {
    /// A rate limit or a server error: the api is there, busy or failing,
    /// and may say when to ask again.
    Refused { retry_after: Option<Duration> },
    /// No api to ask: no connection, or the ingress's 404 while the api's one
    /// pod is replaced (every deploy, for half a minute or so). The status
    /// route answers every device id, an unknown one included, so a 404 that
    /// goes on is routing — a wrong api url, or a proxy in the way — never the
    /// api not knowing the device; that is a 200 saying `unknown`.
    Unreachable,
    /// A refusal about this device.
    Final,
}

impl Unanswered {
    fn from_status(status: reqwest::StatusCode, retry_after: Option<Duration>) -> Unanswered {
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            Unanswered::Refused { retry_after }
        } else if status == reqwest::StatusCode::NOT_FOUND {
            Unanswered::Unreachable
        } else {
            Unanswered::Final
        }
    }
}

/// How often a device showing a pairing code asks whether it is approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalPolls {
    /// The wait between asks while the code is fresh.
    pub fast: Duration,
    /// How long the code counts as fresh.
    pub fast_for: Duration,
    /// The wait between asks after that, and the first wait after a refusal.
    pub then: Duration,
    /// How long an api out of reach is waited for before the pairing ends.
    pub ride_out: Duration,
}

impl ApprovalPolls {
    /// Every second for the two minutes a human typically takes to approve a
    /// code they were just shown, so the approval is seen within a second of
    /// the click (#321); every 2 s after, for a code left waiting. At 60 asks
    /// a minute, a tenth of the 600 the api allows one address. An api out of
    /// reach is waited for as long as a code is fresh, which outlasts any
    /// deploy.
    pub const AFTER_SHOWING_A_CODE: ApprovalPolls = ApprovalPolls {
        fast: Duration::from_secs(1),
        fast_for: Duration::from_secs(120),
        then: Duration::from_secs(2),
        ride_out: Duration::from_secs(120),
    };

    /// The same wait throughout, and an api out of reach ends the pairing at
    /// once.
    pub fn every(interval: Duration) -> ApprovalPolls {
        ApprovalPolls {
            fast: interval,
            fast_for: Duration::ZERO,
            then: interval,
            ride_out: Duration::ZERO,
        }
    }

    /// These polls, waiting `ride_out` for an api out of reach.
    pub fn riding_out(self, ride_out: Duration) -> ApprovalPolls {
        ApprovalPolls { ride_out, ..self }
    }
}

impl From<Duration> for ApprovalPolls {
    fn from(interval: Duration) -> ApprovalPolls {
        ApprovalPolls::every(interval)
    }
}

/// The longest `Retry-After` a refused ask waits out. Far shorter than the
/// beat's: a human may be typing the code in now, and one 429 should not leave
/// it unasked about for minutes.
const LONGEST_RETRY_AFTER: Duration = Duration::from_secs(60);

/// How many answers in a row saying the registration is gone end the wait.
/// More than one, so an answer racing the registration's own write is not
/// taken as the api's last word.
pub const GONE_ANSWERS_BEFORE_ENDING: u32 = 3;

/// What the api said happened to a registration it calls gone, or None while
/// it may yet be approved. Only a pending identity is polled — an approved one
/// is never asked here, and #320's retiring of a lapsed approval runs before
/// any of this — so `revoked` and `unknown` here are about this code.
fn registration_gone(status: &StatusResponse) -> Option<&'static str> {
    match status.state {
        Some(RegistrationState::Unknown) => Some("has no record of this device"),
        Some(RegistrationState::Revoked) => Some("says this device was revoked"),
        _ => None,
    }
}

/// The longest wait between asks while the api keeps refusing or stays out of
/// reach.
const LONGEST_SETBACK_WAIT: Duration = Duration::from_secs(30);

/// The wait before each ask, by what the last one got back. An answer keeps
/// the [`ApprovalPolls`] cadence. A setback — a refusal, or no api — drops to
/// the slow cadence and doubles it with each setback after, up to
/// [`LONGEST_SETBACK_WAIT`], or waits out a `Retry-After` when the api names
/// one; only an answer brings the fast cadence back. An api out of reach for
/// longer than `ride_out` ends the pairing.
#[derive(Debug)]
struct ApprovalPacing {
    polls: ApprovalPolls,
    /// Setbacks since the last answer.
    setbacks: u32,
    /// When, after the code was shown, the api went out of reach.
    unreachable_since: Option<Duration>,
}

impl ApprovalPacing {
    fn new(polls: ApprovalPolls) -> ApprovalPacing {
        ApprovalPacing {
            polls,
            setbacks: 0,
            unreachable_since: None,
        }
    }

    /// The api answered, `elapsed` after the code was shown.
    fn after_answer(&mut self, elapsed: Duration) -> Duration {
        self.setbacks = 0;
        self.unreachable_since = None;
        if elapsed < self.polls.fast_for {
            self.polls.fast
        } else {
            self.polls.then
        }
    }

    /// A status call came back `setback`, `elapsed` after the code was shown;
    /// None when the api has been out of reach for longer than it is waited
    /// for. A `Final` setback is never waited out, and is not asked about.
    fn after_setback(&mut self, setback: &Unanswered, elapsed: Duration) -> Option<Duration> {
        let grown = self
            .polls
            .then
            .saturating_mul(1 << self.setbacks.min(16))
            .min(LONGEST_SETBACK_WAIT);
        self.setbacks = self.setbacks.saturating_add(1);
        match setback {
            Unanswered::Refused { retry_after } => {
                self.unreachable_since = None;
                let asked = retry_after.unwrap_or_default();
                Some(grown.max(asked.min(LONGEST_RETRY_AFTER)))
            }
            Unanswered::Unreachable => {
                let since = *self.unreachable_since.get_or_insert(elapsed);
                (elapsed.saturating_sub(since) < self.polls.ride_out).then_some(grown)
            }
            Unanswered::Final => None,
        }
    }
}

/// Poll the device's status on `polls` until it is approved; returns the
/// owner user id once it is, or ends once the api has said a few times in a
/// row that the registration is gone ([`registration_gone`]). A human may be approving the code right now, so
/// a busy or failing api is asked again, less often while it goes on, and an
/// api out of reach is waited for a while ([`ApprovalPacing`]).
pub async fn poll_until_approved(
    client: &reqwest::Client,
    api_url: &str,
    device_id: &str,
    polls: impl Into<ApprovalPolls>,
) -> Result<String> {
    let mut pacing = ApprovalPacing::new(polls.into());
    let shown = tokio::time::Instant::now();
    let mut gone_answers = 0;
    loop {
        let wait = match ask_status(client, api_url, device_id).await {
            Ok(status) if status.approved => return Ok(status.owner_user_id.unwrap_or_default()),
            Ok(status) => {
                gone_answers = gone_answers_after(gone_answers, &status)?;
                pacing.after_answer(shown.elapsed())
            }
            Err((error, setback)) => pacing
                .after_setback(&setback, shown.elapsed())
                .ok_or(error)?,
        };
        tokio::time::sleep(wait).await;
    }
}

/// The count of answers in a row calling the registration gone, after one
/// more answer `status`; the pairing's end once there are enough.
fn gone_answers_after(before: u32, status: &StatusResponse) -> Result<u32> {
    let Some(why) = registration_gone(status) else {
        return Ok(0);
    };
    let now = before + 1;
    if now >= GONE_ANSWERS_BEFORE_ENDING {
        return Err(PairingError::PairingEnded(why));
    }
    Ok(now)
}

/// Retire the stored identity if it says approved and the api disagrees, so
/// the pairing that follows mints a new device id and new keys and a human has
/// to approve a fresh code. An identity that is missing or still pending is
/// left alone without asking (pairing re-registers a pending one with a new
/// code), and an api that cannot answer retires nothing: only an answer of
/// "not approved" does.
///
/// New keys rather than the old ones: a device is revoked because it was lost
/// or its keys may have leaked, and a fresh approval of the old key would
/// re-trust whoever else holds it.
///
/// Only the api that approved the identity is believed (#320): an agent's mock
/// behind `BRIDGE_API_URL` once retired a machine's production pairing on one
/// answer. Another api's "not approved" is [`PairingError::ApprovedElsewhere`],
/// unless `when` is [`RetireWhen::AnyApiSays`] (`pair --retire`).
pub async fn retire_lapsed_approval(
    client: &reqwest::Client,
    api_url: &str,
    identity_path: &Path,
    when: RetireWhen,
) -> Result<Option<RetiredApproval>> {
    let file_error = |e: identity::IdentityError| {
        PairingError::Identity(format!("{}: {e}", identity_path.display()))
    };
    let stored = match identity::load(identity_path).map_err(file_error)? {
        Some(stored) if stored.approved => stored,
        _ => return Ok(None),
    };
    let status = fetch_status(client, api_url, &stored.device_id).await?;
    let Some(lapse) = status.lapse() else {
        return Ok(None);
    };
    if when == RetireWhen::ApproverSays && !may_retire(&stored, api_url) {
        return Err(PairingError::ApprovedElsewhere {
            asked: api_url.to_string(),
            approver: stored.approved_by.clone(),
        });
    }
    let kept_at = identity::retire(identity_path, &stored).map_err(file_error)?;
    Ok(Some(RetiredApproval {
        lapse,
        api_url: api_url.to_string(),
        identity_path: identity_path.to_path_buf(),
        kept_at,
    }))
}

/// Whose "not approved" retires a stored approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetireWhen {
    /// Only the api that approved it: the default.
    ApproverSays,
    /// Whichever api is configured: `pair --retire`.
    AnyApiSays,
}

/// Whether `api_url`'s "not approved" may retire `stored` without `--retire`:
/// it is the api `stored` records as its approver, or, for an identity that
/// records none, the default api, the only one the installer pairs with.
/// Only a pairing an api completes records it as approver: an api that merely
/// says an approved identity is approved never does, or a mock's "approved"
/// then "not approved" would retire a good identity (#320).
fn may_retire(stored: &StoredIdentity, api_url: &str) -> bool {
    let approver = stored
        .approved_by
        .as_deref()
        .unwrap_or(crate::config::DEFAULT_API_URL);
    same_api(approver, api_url)
}

/// An api url with any trailing slash dropped, as it is recorded.
fn api_key(api_url: &str) -> &str {
    api_url.trim_end_matches('/')
}

/// Whether two api urls name the same api: the same scheme, host (any case)
/// and port (written or the scheme's default), and the same path but for a
/// trailing slash. Urls that do not parse are compared as written.
pub fn same_api(a: &str, b: &str) -> bool {
    fn parts(url: &str) -> Option<(String, String, u16, String)> {
        let parsed = reqwest::Url::parse(url).ok()?;
        Some((
            parsed.scheme().to_string(),
            parsed.host_str()?.to_ascii_lowercase(),
            parsed.port_or_known_default()?,
            parsed.path().trim_end_matches('/').to_string(),
        ))
    }
    match (parts(a), parts(b)) {
        (Some(a), Some(b)) => a == b,
        _ => api_key(a) == api_key(b),
    }
}

/// Ensure the device is registered and approved. If `stored.approved`, returns it
/// unchanged with no network calls. Otherwise: register as pending, print the pairing
/// code + fingerprint + approve URL, poll until approved, persist `approved = true`,
/// and return the updated identity. Takes ownership and returns the approved copy.
///
/// `pairing_code_override` (env `BRIDGE_PAIRING_CODE`) replaces the random code so
/// dev/compose automation can complete the real approve flow with a known code;
/// leave it `None` (the default everywhere humans pair) for a fresh random code.
pub async fn ensure_paired(
    client: &reqwest::Client,
    api_url: &str,
    web_url: &str,
    identity_path: &Path,
    mut stored: StoredIdentity,
    polls: impl Into<ApprovalPolls>,
    pairing_code_override: Option<&str>,
) -> Result<StoredIdentity> {
    if stored.approved {
        return Ok(stored);
    }

    let pairing_code = match pairing_code_override {
        Some(code) => code.to_string(),
        None => generate_pairing_code(),
    };
    let req = build_register_request(&stored, &pairing_code)?;
    register(client, api_url, &req).await?;

    let fingerprint = transport::fingerprint_identity_key(&stored.identity_public_key_b64)
        .map_err(|e| PairingError::Identity(e.to_string()))?;
    eprintln!("{}", pairing_prompt(&pairing_code, &fingerprint, web_url));

    poll_until_approved(client, api_url, &stored.device_id, polls).await?;
    stored.approved = true;
    stored.approved_by = Some(api_key(api_url).to_string());
    identity::save(identity_path, &stored).map_err(|e| PairingError::Identity(e.to_string()))?;
    // What happens next is the caller's business — `serve` connects to the
    // relay, `pair` exits — so pairing reports only the pairing it did.
    eprintln!("{INDENT}Device approved.");
    Ok(stored)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity;

    /// A second between asks for the first two minutes after a code is
    /// shown, 2 s after.
    #[test]
    fn a_fresh_code_is_asked_about_every_second_then_every_two() {
        let mut pacing = ApprovalPacing::new(ApprovalPolls::AFTER_SHOWING_A_CODE);
        let mut answered = |secs: f64| pacing.after_answer(Duration::from_secs_f64(secs));
        assert_eq!(answered(0.0), Duration::from_secs(1));
        assert_eq!(answered(119.9), Duration::from_secs(1));
        assert_eq!(answered(120.0), Duration::from_secs(2));
        assert_eq!(answered(3600.0), Duration::from_secs(2));
        let mut every = ApprovalPacing::new(Duration::from_secs(3).into());
        assert_eq!(every.after_answer(Duration::ZERO), Duration::from_secs(3));
    }

    fn refused(retry_after: Option<u64>) -> Unanswered {
        Unanswered::Refused {
            retry_after: retry_after.map(Duration::from_secs),
        }
    }

    fn waits(pacing: &mut ApprovalPacing, setback: &Unanswered, times: usize) -> Vec<u64> {
        (0..times)
            .map(|_| {
                pacing
                    .after_setback(setback, Duration::ZERO)
                    .unwrap()
                    .as_secs()
            })
            .collect()
    }

    /// A busy or failing api is not asked every second: it drops to the slow
    /// cadence and backs off further each time it refuses again, and the fast
    /// cadence comes back only with an answer (#321 review).
    #[test]
    fn a_refusal_backs_off_until_an_answer() {
        let mut pacing = ApprovalPacing::new(ApprovalPolls::AFTER_SHOWING_A_CODE);
        assert_eq!(waits(&mut pacing, &refused(None), 6), [2, 4, 8, 16, 30, 30]);
        assert_eq!(
            pacing.after_answer(Duration::from_secs(10)),
            Duration::from_secs(1)
        );
        assert_eq!(waits(&mut pacing, &refused(None), 1), [2]);
    }

    /// A refusal that names its own `Retry-After` is not asked again before
    /// it, and never sooner than the slow cadence.
    #[test]
    fn a_refusal_is_asked_again_when_it_says() {
        let mut pacing = ApprovalPacing::new(ApprovalPolls::AFTER_SHOWING_A_CODE);
        assert_eq!(waits(&mut pacing, &refused(Some(7)), 1), [7]);
        assert_eq!(waits(&mut pacing, &refused(Some(0)), 1), [4]);
        // One refusal cannot leave a fresh code unasked about for long.
        assert_eq!(waits(&mut pacing, &refused(Some(86_400)), 1), [60]);
    }

    /// An api out of reach — no connection, or the ingress's 404 while its pod
    /// is replaced — is ridden out on the refusal's backoff for a bounded
    /// time, then ends the pairing. An answer in between starts the bound over.
    #[test]
    fn an_api_out_of_reach_is_ridden_out_for_a_while() {
        let polls = ApprovalPolls::AFTER_SHOWING_A_CODE;
        let mut pacing = ApprovalPacing::new(polls);
        let mut gone =
            |secs: u64| pacing.after_setback(&Unanswered::Unreachable, Duration::from_secs(secs));
        assert_eq!(gone(10), Some(Duration::from_secs(2)));
        assert_eq!(gone(12), Some(Duration::from_secs(4)));
        assert_eq!(
            gone(10 + polls.ride_out.as_secs() - 1),
            Some(Duration::from_secs(8))
        );
        assert_eq!(gone(10 + polls.ride_out.as_secs()), None);

        let mut pacing = ApprovalPacing::new(polls);
        pacing.after_setback(&Unanswered::Unreachable, Duration::from_secs(10));
        pacing.after_answer(Duration::from_secs(100));
        let back = Duration::from_secs(100 + polls.ride_out.as_secs() - 1);
        pacing.after_setback(&Unanswered::Unreachable, Duration::from_secs(101));
        assert!(pacing
            .after_setback(&Unanswered::Unreachable, back)
            .is_some());
    }

    #[test]
    fn a_status_is_sorted_by_whether_it_could_pass() {
        let sorted =
            |code: u16| Unanswered::from_status(reqwest::StatusCode::from_u16(code).unwrap(), None);
        assert!(matches!(sorted(429), Unanswered::Refused { .. }));
        assert!(matches!(sorted(503), Unanswered::Refused { .. }));
        assert!(matches!(sorted(404), Unanswered::Unreachable));
        assert!(matches!(sorted(403), Unanswered::Final));
        assert!(matches!(sorted(400), Unanswered::Final));
    }

    fn status(json: serde_json::Value) -> StatusResponse {
        serde_json::from_value(json).unwrap()
    }

    /// While a code is shown, only an api that says the registration is gone
    /// — revoked, or no such device — ends the wait; pending, an older api
    /// that says nothing, and a state this bridge does not know keep it.
    #[test]
    fn only_a_registration_the_api_calls_gone_ends_the_wait() {
        let gone = |json| registration_gone(&status(json));
        assert!(gone(serde_json::json!({"approved": false, "state": "unknown"})).is_some());
        assert!(gone(serde_json::json!({"approved": false, "state": "revoked"})).is_some());
        assert!(gone(serde_json::json!({"approved": false, "state": "pending"})).is_none());
        assert!(gone(serde_json::json!({"approved": false})).is_none());
        assert!(gone(serde_json::json!({"approved": false, "state": "suspended"})).is_none());
    }

    #[test]
    fn an_approved_device_has_not_lapsed() {
        let approved = status(serde_json::json!({
            "approved": true, "owner_user_id": "u1", "state": "approved"
        }));
        assert_eq!(approved.lapse(), None);
    }

    #[test]
    fn the_api_says_which_way_an_approval_lapsed() {
        let revoked = status(serde_json::json!({"approved": false, "state": "revoked"}));
        let unknown = status(serde_json::json!({"approved": false, "state": "unknown"}));
        assert_eq!(revoked.lapse(), Some(Lapse::Revoked));
        assert_eq!(unknown.lapse(), Some(Lapse::Unknown));
    }

    /// An api from before `state` (and any state this bridge does not know)
    /// says only "not approved": the approval lapsed, reason unknown.
    #[test]
    fn an_api_that_does_not_say_why_still_reads_as_lapsed() {
        let old_api = status(serde_json::json!({"approved": false, "owner_user_id": null}));
        let newer_state = status(serde_json::json!({"approved": false, "state": "suspended"}));
        let pending = status(serde_json::json!({"approved": false, "state": "pending"}));
        assert_eq!(old_api.lapse(), Some(Lapse::NotApproved));
        assert_eq!(newer_state.lapse(), Some(Lapse::NotApproved));
        assert_eq!(pending.lapse(), Some(Lapse::NotApproved));
    }

    #[test]
    fn a_lapse_reads_as_a_reason_the_operator_can_act_on() {
        assert!(Lapse::Revoked
            .to_string()
            .contains("revoked in Settings → Devices"));
        assert!(Lapse::Unknown
            .to_string()
            .contains("no longer has this device"));
        assert!(Lapse::NotApproved
            .to_string()
            .contains("no longer approved"));
    }

    /// The installer's terminal is assumed 80 columns wide; nothing the
    /// pairing flow prints may rely on the terminal wrapping it (#319).
    const TERMINAL_COLUMNS: usize = 80;

    fn assert_fits(text: &str) {
        for line in text.lines() {
            assert!(
                line.chars().count() <= TERMINAL_COLUMNS,
                "{} columns: {line:?}",
                line.chars().count()
            );
        }
    }

    const FINGERPRINT: &str = "28e679939bc32c446627fac8a8dd58a5353e66b744a9a2dae91254f1da1b9027";

    #[test]
    fn the_approve_link_opens_the_pairing_screen_with_the_code_in_the_fragment() {
        assert_eq!(
            approve_url("https://getbuild.ing/", "ZSAC-ABU6"),
            "https://getbuild.ing/app/#/pair/ZSAC-ABU6"
        );
        assert_eq!(
            approve_url("http://localhost:8090", "ZSAC-ABU6"),
            "http://localhost:8090/app/#/pair/ZSAC-ABU6"
        );
    }

    /// The short form is the one the approve screen shows beside the device.
    #[test]
    fn the_short_fingerprint_is_its_first_thirty_two_hex_digits_in_fours() {
        assert_eq!(
            short_fingerprint(FINGERPRINT),
            "28e6 7993 9bc3 2c44 6627 fac8 a8dd 58a5"
        );
    }

    #[test]
    fn the_pairing_prompt_is_one_link_a_code_and_a_short_fingerprint() {
        let prompt = pairing_prompt("ZSAC-ABU6", FINGERPRINT, "https://getbuild.ing");
        assert_fits(&prompt);
        assert!(prompt.contains("ZSAC-ABU6"), "{prompt}");
        assert!(
            prompt.contains("fingerprint:   28e6 7993 9bc3 2c44 6627 fac8 a8dd 58a5\n"),
            "{prompt}"
        );
        assert!(!prompt.contains(FINGERPRINT), "{prompt}");
        assert!(
            prompt.contains("approve at:    https://getbuild.ing/app/#/pair/ZSAC-ABU6"),
            "{prompt}"
        );
        assert!(!prompt.contains("Settings"), "{prompt}");
        assert!(prompt.contains("Waiting for you to approve it"), "{prompt}");
    }

    fn retired_at(api_url: &str, home: &Path) -> RetiredApproval {
        let identity_path = home.join(".build/identity.json");
        RetiredApproval {
            lapse: Lapse::Unknown,
            api_url: api_url.into(),
            kept_at: identity_path
                .with_file_name("identity.json.retired-b02b5ba1-2483-41c8-99a8-08963e40991a"),
            identity_path,
        }
    }

    /// The notice from the photo in #319, at the installer's width: what is
    /// happening and where the old identity went, nothing more.
    #[test]
    fn a_retired_approval_at_the_default_api_says_what_happened_and_where_it_went() {
        let home = Path::new("/Users/zechariahzimmerman");
        let notice = retired_at(crate::config::DEFAULT_API_URL, home).notice(home);
        assert_fits(&notice);
        assert_eq!(
            notice,
            "    This machine's earlier pairing is no longer valid.\n    \
             Pairing it as a new device; the old identity is kept in ~/.build."
        );
    }

    /// A wrong `BRIDGE_API_URL` is the one way a good identity gets retired,
    /// so an api other than the default is named, on a line of its own with
    /// nothing glued to the url.
    #[test]
    fn a_retired_approval_names_an_api_that_is_not_the_default() {
        let home = Path::new("/home/dev");
        let notice = retired_at("http://localhost:8090", home).notice(home);
        assert_fits(&notice);
        assert!(
            notice.contains("\n    Asked http://localhost:8090"),
            "{notice}"
        );
        assert!(!notice.contains("8090."), "{notice}");
    }

    #[test]
    fn a_long_api_url_still_fits_and_stands_alone() {
        let home = Path::new("/home/dev");
        let api = "https://bridge-api.staging.internal.example-company.cloud/build/v2/eu-west-1";
        let notice = retired_at(api, home).notice(home);
        assert_fits(&notice);
        assert!(notice.lines().any(|line| line.trim() == api), "{notice}");
    }

    /// An identity outside the home directory is named by its full directory,
    /// broken onto its own line when the sentence would not fit.
    #[test]
    fn a_retired_approval_outside_home_names_the_full_directory() {
        let home = Path::new("/home/dev");
        let notice = retired_at(
            crate::config::DEFAULT_API_URL,
            Path::new("/var/lib/build-bridge"),
        )
        .notice(home);
        assert_fits(&notice);
        assert!(
            notice
                .split_whitespace()
                .any(|word| word == "/var/lib/build-bridge/.build."),
            "{notice}"
        );
    }

    /// A long web url puts the link on a line of its own, whole.
    #[test]
    fn a_long_web_url_gets_the_link_a_line_of_its_own() {
        let web = "https://build.staging.internal.example-company.cloud/teams/platform";
        let prompt = pairing_prompt("ZSAC-ABU6", FINGERPRINT, web);
        assert_fits(&prompt.replace(&approve_url(web, "ZSAC-ABU6"), ""));
        assert!(prompt.contains("\n    approve at:\n"), "{prompt}");
        assert!(
            prompt.contains(&format!("\n      {}\n", approve_url(web, "ZSAC-ABU6"))),
            "{prompt}"
        );
    }

    /// A refusal is one sentence of any length; printed, it breaks between
    /// words into indented lines, and a word longer than a line stands alone.
    /// An identity that records no approver is the default api's to retire,
    /// and no other api's (#320).
    #[test]
    fn without_a_recorded_approver_only_the_default_api_may_retire() {
        let legacy = identity::generate("my-box");
        assert!(may_retire(&legacy, crate::config::DEFAULT_API_URL));
        assert!(!may_retire(&legacy, "http://localhost:8090"));
    }

    /// The same api however its url is written: scheme, host in any case,
    /// port written or implied, trailing slash or not (#320).
    #[test]
    fn same_api_compares_scheme_host_and_effective_port() {
        let default = crate::config::DEFAULT_API_URL;
        for same in [
            "https://getbuild.ing",
            "https://getbuild.ing/",
            "https://GetBuild.ing",
            "HTTPS://GETBUILD.ING/",
            "https://getbuild.ing:443",
            "https://getbuild.ing:443/",
        ] {
            assert!(same_api(same, default), "{same}");
            assert!(same_api(default, same), "{same}");
        }
        for other in [
            "http://getbuild.ing",
            "https://getbuild.ing:8443",
            "https://staging.getbuild.ing",
            "https://getbuild.ing/v2",
            "http://localhost:8090",
        ] {
            assert!(!same_api(other, default), "{other}");
        }
        assert!(same_api("http://localhost:80/", "http://LOCALHOST"));
        assert!(same_api("http://app:8080/api/", "http://app:8080/api"));
        assert!(same_api("not a url/", "not a url"));
        assert!(!same_api("not a url", default));
    }

    #[test]
    fn wrapped_breaks_between_words_under_an_indent() {
        let reason = "not paired: could not confirm this device's pairing with the api \
                      (error sending request for url (https://getbuild.ing/api/devices/\
                      b02b5ba1-2483-41c8-99a8-08963e40991a/status)) — check the connection";
        let text = wrapped(reason, "    ");
        let long_word = text
            .lines()
            .find(|line| line.trim().starts_with("(https://"))
            .expect("the url is a line of its own");
        assert!(!long_word.trim().contains(' '), "{text}");
        assert_fits(&text.replace(long_word, ""));
        assert!(text.lines().all(|line| line.starts_with("    ")), "{text}");
        assert_eq!(
            text.split_whitespace().collect::<Vec<_>>(),
            reason.split_whitespace().collect::<Vec<_>>()
        );
        assert_eq!(wrapped("short", "  "), "  short");
    }

    #[test]
    fn pairing_code_is_high_entropy_and_distinct() {
        let a = generate_pairing_code();
        let b = generate_pairing_code();
        assert_ne!(a, b, "two codes should differ");
        assert_eq!(a.len(), CODE_LEN + 1, "code has a dash separator");
        assert!(a.contains('-'));
        // Every non-dash char is from the unambiguous alphabet.
        assert!(a
            .chars()
            .filter(|c| *c != '-')
            .all(|c| CODE_ALPHABET.contains(&(c as u8))));
    }

    #[test]
    fn hash_pairing_code_is_sha256_hex_known_answer() {
        // SHA-256 of "abc" is a well-known constant.
        assert_eq!(
            hash_pairing_code("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn registration_challenge_binds_all_fields() {
        let base = registration_challenge("d", "idpub", "tppub", "hash");
        assert_ne!(base, registration_challenge("D", "idpub", "tppub", "hash"));
        assert_ne!(base, registration_challenge("d", "OTHER", "tppub", "hash"));
        assert_ne!(base, registration_challenge("d", "idpub", "OTHER", "hash"));
        assert_ne!(base, registration_challenge("d", "idpub", "tppub", "OTHER"));
    }

    #[test]
    fn build_register_request_signature_verifies() {
        let id = identity::generate("my-box");
        let req = build_register_request(&id, "WXYZ-4F2K").unwrap();
        let challenge = registration_challenge(
            &req.device_id,
            &req.identity_public_key_b64,
            &req.transport_public_key_b64,
            &req.pairing_code_hash,
        );
        transport::verify_message_b64(
            &req.identity_public_key_b64,
            challenge.as_bytes(),
            &req.signature_b64,
        )
        .expect("registration signature verifies against the device identity key");
    }

    #[test]
    fn build_register_request_tampered_code_hash_fails_verification() {
        let id = identity::generate("my-box");
        let req = build_register_request(&id, "WXYZ-4F2K").unwrap();
        // Verify against a challenge built with a different code hash → must fail.
        let forged = registration_challenge(
            &req.device_id,
            &req.identity_public_key_b64,
            &req.transport_public_key_b64,
            "tampered-hash",
        );
        assert!(transport::verify_message_b64(
            &req.identity_public_key_b64,
            forged.as_bytes(),
            &req.signature_b64,
        )
        .is_err());
    }

    #[test]
    fn build_register_request_never_includes_raw_code() {
        let id = identity::generate("my-box");
        let code = "WXYZ-4F2K";
        let req = build_register_request(&id, code).unwrap();
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            !json.contains(code),
            "raw pairing code must not be serialized"
        );
        assert!(json.contains(&hash_pairing_code(code)));
    }
}
