//! Claude Code's credential context, shared by its TUI and its headless
//! carrier.
//!
//! `claude auth status` (JSON by default) is read through an allowlist —
//! `loggedIn`, `authMethod`, `apiKeySource` — and the rest of its answer
//! (email, organisation, config paths) is dropped unread. It is run only
//! where no settings file names a helper command: the inspected 2.1.284
//! status path resolves an API key through `apiKeyHelper` when one is set,
//! which would run it. Its token sources are synchronous reads of saved
//! credentials, with no refresh. The OAuth expiry comes from the saved
//! credentials file, read for `expiresAt` and the presence of each token.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use super::{oauth_status, read_json, summarise, AuthAdapter, ObservationFailed, Present};
use crate::harness::installed::executable::Executable;
use crate::harness::installed::probe::child::ProbeChild;
use crate::harness::installed::probe::PROBE_DEADLINE;
use crate::harness::inventory::environment::DeviceEnvironment;
use crate::harness::inventory::model::{AuthFacts, AuthMethod, AuthStatus, Evidence};

pub struct ClaudeAuth;

pub static CLAUDE_AUTH: ClaudeAuth = ClaudeAuth;

/// Settings that name a command Claude Code runs to get or refresh a
/// credential. Any of them keeps the status command from running at all.
#[derive(Debug, Default, Deserialize)]
struct Settings {
    #[serde(default, rename = "apiKeyHelper")]
    api_key_helper: Present,
    #[serde(default, rename = "awsAuthRefresh")]
    aws_auth_refresh: Present,
    #[serde(default, rename = "awsCredentialExport")]
    aws_credential_export: Present,
    #[serde(default, rename = "gcpAuthRefresh")]
    gcp_auth_refresh: Present,
    #[serde(default, rename = "otelHeadersHelper")]
    otel_headers_helper: Present,
    /// Variables the settings set for every session, by name.
    #[serde(default)]
    env: std::collections::BTreeMap<String, Present>,
}

impl Settings {
    fn names_a_helper(&self) -> bool {
        [
            self.api_key_helper,
            self.aws_auth_refresh,
            self.aws_credential_export,
            self.gcp_auth_refresh,
            self.otel_headers_helper,
        ]
        .iter()
        .any(|helper| helper.0)
    }
}

#[derive(Debug, Default, Deserialize)]
struct CredentialsFile {
    #[serde(default, rename = "claudeAiOauth")]
    oauth: Option<OauthCredentials>,
}

#[derive(Debug, Default, Deserialize)]
struct OauthCredentials {
    #[serde(default, rename = "accessToken")]
    access: Present,
    #[serde(default, rename = "refreshToken")]
    refresh: Present,
    #[serde(default, rename = "expiresAt")]
    expires_at: Option<u64>,
}

/// The allowlisted part of `claude auth status`.
#[derive(Debug, Deserialize)]
struct StatusAnswer {
    #[serde(rename = "loggedIn")]
    logged_in: bool,
    #[serde(rename = "authMethod")]
    auth_method: String,
    #[serde(default, rename = "apiKeySource")]
    api_key_source: Option<String>,
}

/// What the device's settings and environment say before the CLI is asked.
struct Saved {
    settings: Vec<Settings>,
    oauth: Option<OauthCredentials>,
}

impl Saved {
    fn read(environment: &DeviceEnvironment) -> Self {
        let config = config_dir(environment);
        Saved {
            settings: settings_paths(environment)
                .iter()
                .filter_map(|path| read_json(path))
                .collect(),
            oauth: read_json::<CredentialsFile>(&config.join(".credentials.json"))
                .and_then(|file| file.oauth)
                .filter(|oauth| oauth.access.0 || oauth.refresh.0),
        }
    }

    /// Where `name` is set for a session: the environment, or a settings
    /// file's `env`.
    fn sets(&self, environment: &DeviceEnvironment, name: &str) -> Option<Evidence> {
        if environment.has(name) {
            return Some(Evidence::Environment);
        }
        self.settings
            .iter()
            .any(|settings| settings.env.get(name).is_some_and(|value| value.0))
            .then_some(Evidence::Settings)
    }

    fn oauth_status(&self, now: SystemTime) -> Option<AuthStatus> {
        self.oauth
            .as_ref()
            .map(|oauth| oauth_status(oauth.expires_at, oauth.refresh.0, now))
    }
}

const EXTERNAL_PROVIDERS: [&str; 3] = [
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];
const LONG_LIVED_TOKENS: [&str; 2] = ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_AUTH_TOKEN"];

fn config_dir(environment: &DeviceEnvironment) -> PathBuf {
    environment.dir_or_home("CLAUDE_CONFIG_DIR", ".claude")
}

fn settings_paths(environment: &DeviceEnvironment) -> Vec<PathBuf> {
    let config = config_dir(environment);
    [
        config.join("settings.json"),
        config.join("settings.local.json"),
    ]
    .into_iter()
    .chain(environment.claude_managed_settings().iter().cloned())
    .collect()
}

impl AuthAdapter for ClaudeAuth {
    fn context(&self) -> &'static str {
        "claude"
    }

    fn watched(&self, environment: &DeviceEnvironment) -> Vec<PathBuf> {
        let config = config_dir(environment);
        let mut watched = settings_paths(environment);
        watched.push(config.join(".credentials.json"));
        watched.push(environment.home().join(".claude.json"));
        watched
    }

    fn observe(
        &self,
        environment: &DeviceEnvironment,
        executable: Option<&Executable>,
        now: SystemTime,
    ) -> Result<AuthFacts, ObservationFailed> {
        let saved = Saved::read(environment);
        if saved.settings.iter().any(Settings::names_a_helper) {
            // Not run: its answer could cost a helper command.
            return Ok(AuthFacts::saved(
                AuthMethod::External,
                AuthStatus::Unknown,
                vec![Evidence::Settings],
            ));
        }
        match executable {
            Some(executable) => {
                let answer = ask_status(environment, executable)?;
                Ok(from_status(&answer, &saved, now))
            }
            None => Ok(from_saved(environment, &saved, now)),
        }
    }
}

fn ask_status(
    environment: &DeviceEnvironment,
    executable: &Executable,
) -> Result<StatusAnswer, ObservationFailed> {
    let mut command = environment.probe_command(executable.path(), &["auth", "status"]);
    command
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_AUTOUPDATER", "1");
    let mut child = ProbeChild::spawn(command, PROBE_DEADLINE)
        .map_err(|_| ObservationFailed("claude auth status did not start"))?;
    let mut said = String::new();
    while let Some(line) = child
        .next_line()
        .map_err(|_| ObservationFailed("claude auth status did not answer in time"))?
    {
        said.push_str(&line);
        said.push('\n');
    }
    // Signed out exits 1 with the same JSON; either is an answer.
    child
        .succeeded()
        .map_err(|_| ObservationFailed("claude auth status did not exit in time"))?;
    serde_json::from_str(&said)
        .map_err(|_| ObservationFailed("claude auth status answered in words Build cannot read"))
}

fn from_status(answer: &StatusAnswer, saved: &Saved, now: SystemTime) -> AuthFacts {
    let mut evidence = vec![Evidence::CliStatus];
    let (method, status) = match answer.auth_method.as_str() {
        "claude.ai" => {
            if saved.oauth.is_some() {
                evidence.push(Evidence::CredentialsFile);
            }
            let method = if answer.api_key_source.as_deref() == Some("ANTHROPIC_API_KEY") {
                AuthMethod::Mixed
            } else {
                AuthMethod::Oauth
            };
            (
                method,
                saved.oauth_status(now).unwrap_or(AuthStatus::SignedIn),
            )
        }
        "oauth_token" => (AuthMethod::LongLivedToken, AuthStatus::SignedIn),
        "api_key" => (AuthMethod::ApiKey, AuthStatus::SignedIn),
        "api_key_helper" | "third_party" => (AuthMethod::External, AuthStatus::SignedIn),
        "none" => (AuthMethod::None, AuthStatus::NotSignedIn),
        _ => (AuthMethod::Unknown, AuthStatus::Unknown),
    };
    let status = match (answer.logged_in, status) {
        (false, AuthStatus::SignedIn) => AuthStatus::NotSignedIn,
        (_, status) => status,
    };
    AuthFacts::saved(method, status, evidence)
}

/// No CLI to ask: what the saved credentials and the environment say.
fn from_saved(environment: &DeviceEnvironment, saved: &Saved, now: SystemTime) -> AuthFacts {
    if let Some(evidence) = EXTERNAL_PROVIDERS
        .iter()
        .find_map(|name| saved.sets(environment, name))
    {
        return AuthFacts::saved(AuthMethod::External, AuthStatus::Unknown, vec![evidence]);
    }
    let mut parts = Vec::new();
    let mut evidence = Vec::new();
    let mut note = |method, status, source| {
        parts.push((method, status));
        if !evidence.contains(&source) {
            evidence.push(source);
        }
    };
    if let Some(source) = LONG_LIVED_TOKENS
        .iter()
        .find_map(|name| saved.sets(environment, name))
    {
        note(AuthMethod::LongLivedToken, AuthStatus::SignedIn, source);
    }
    if let Some(source) = saved.sets(environment, "ANTHROPIC_API_KEY") {
        note(AuthMethod::ApiKey, AuthStatus::SignedIn, source);
    }
    if let Some(status) = saved.oauth_status(now) {
        note(AuthMethod::Oauth, status, Evidence::CredentialsFile);
    }
    let (method, status) = summarise(&parts);
    AuthFacts::saved(method, status, evidence)
}
