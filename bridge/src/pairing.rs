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
        let mut notice = format!(
            "    This machine's earlier pairing is no longer valid.\n    \
             Pairing it as a new device; the old identity is kept in {}.",
            home_shortened(kept_in, home)
        );
        if self.api_url.trim_end_matches('/') != crate::config::DEFAULT_API_URL {
            notice.push_str(&format!("\n    Asked {}.", self.api_url));
        }
        notice
    }
}

/// `text` broken between words into lines of at most 80 columns, each
/// starting with `indent`. A word too long for a line gets one to itself.
pub fn wrapped(text: &str, indent: &str) -> String {
    const COLUMNS: usize = 80;
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

/// The fingerprint as the approve screen shows it beside the device: its first
/// sixteen hex digits, in fours.
pub fn short_fingerprint(fingerprint: &str) -> String {
    let digits: Vec<char> = fingerprint.chars().take(16).collect();
    digits
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The block `pair` prints while it waits, indented under the installer's
/// step: one link that opens the approve screen with this device pulled up,
/// and the code and fingerprint to compare there.
pub fn pairing_prompt(code: &str, fingerprint: &str, web_url: &str) -> String {
    format!(
        "\n    pairing code:  {code}\n    fingerprint:   {}\n    approve at:    {}\n\n    \
         Waiting for you to approve it in Build…",
        short_fingerprint(fingerprint),
        approve_url(web_url, code)
    )
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
    let url = format!(
        "{}/api/devices/{device_id}/status",
        api_url.trim_end_matches('/')
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| PairingError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(PairingError::Rejected(resp.status().to_string()));
    }
    resp.json::<StatusResponse>()
        .await
        .map_err(|e| PairingError::Http(e.to_string()))
}

/// Poll `fetch_status` every `interval` until the device is approved; returns the
/// owner user id once it is.
pub async fn poll_until_approved(
    client: &reqwest::Client,
    api_url: &str,
    device_id: &str,
    interval: Duration,
) -> Result<String> {
    loop {
        let status = fetch_status(client, api_url, device_id).await?;
        if status.approved {
            return Ok(status.owner_user_id.unwrap_or_default());
        }
        tokio::time::sleep(interval).await;
    }
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
pub async fn retire_lapsed_approval(
    client: &reqwest::Client,
    api_url: &str,
    identity_path: &Path,
) -> Result<Option<RetiredApproval>> {
    let file_error = |e: identity::IdentityError| {
        PairingError::Identity(format!("{}: {e}", identity_path.display()))
    };
    let stored = match identity::load(identity_path).map_err(file_error)? {
        Some(stored) if stored.approved => stored,
        _ => return Ok(None),
    };
    let Some(lapse) = fetch_status(client, api_url, &stored.device_id)
        .await?
        .lapse()
    else {
        return Ok(None);
    };
    let kept_at = identity::retire(identity_path, &stored).map_err(file_error)?;
    Ok(Some(RetiredApproval {
        lapse,
        api_url: api_url.to_string(),
        identity_path: identity_path.to_path_buf(),
        kept_at,
    }))
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
    poll_interval: Duration,
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

    poll_until_approved(client, api_url, &stored.device_id, poll_interval).await?;
    stored.approved = true;
    identity::save(identity_path, &stored).map_err(|e| PairingError::Identity(e.to_string()))?;
    // What happens next is the caller's business — `serve` connects to the
    // relay, `pair` exits — so pairing reports only the pairing it did.
    eprintln!("    Device approved.");
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

    fn status(json: serde_json::Value) -> StatusResponse {
        serde_json::from_value(json).unwrap()
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
    fn the_short_fingerprint_is_its_first_sixteen_hex_digits_in_fours() {
        assert_eq!(short_fingerprint(FINGERPRINT), "28e6 7993 9bc3 2c44");
    }

    #[test]
    fn the_pairing_prompt_is_one_link_a_code_and_a_short_fingerprint() {
        let prompt = pairing_prompt("ZSAC-ABU6", FINGERPRINT, "https://getbuild.ing");
        assert_fits(&prompt);
        assert!(prompt.contains("ZSAC-ABU6"), "{prompt}");
        assert!(prompt.contains("28e6 7993 9bc3 2c44"), "{prompt}");
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
    fn a_retired_approval_says_so_in_two_short_lines() {
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
    /// so an api other than the default is named.
    #[test]
    fn a_retired_approval_names_an_api_that_is_not_the_default() {
        let home = Path::new("/home/dev");
        let notice = retired_at("http://localhost:8090", home).notice(home);
        assert_fits(&notice);
        assert!(notice.contains("Asked http://localhost:8090"), "{notice}");
    }

    /// An identity outside the home directory is named by its directory.
    #[test]
    fn a_retired_approval_outside_home_names_the_full_directory() {
        let home = Path::new("/home/dev");
        let notice = retired_at(crate::config::DEFAULT_API_URL, Path::new("/srv/bridge")).notice(home);
        assert!(notice.contains("kept in /srv/bridge/.build."), "{notice}");
    }

    /// A refusal is one sentence of any length; printed, it breaks between
    /// words into indented lines, and a word longer than a line stands alone.
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
