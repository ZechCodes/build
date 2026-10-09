//! Claude Code's credential context, shared by its TUI and its headless
//! carrier.
//!
//! Metadata only: Claude Code is never run to observe it. Its `auth status`
//! goes through the root pre-action hook, which in 2.1.284 schedules an
//! OAuth refresh and can run `policyHelper`s before any auth-specific guard
//! (#466). So the method and status come from the saved credentials file —
//! `expiresAt` and which tokens it holds, never their values — the global
//! config's `primaryApiKey` (an API key saved by `/login`), and from the
//! settings and environment a session reads, by name: an `apiKeyHelper` in
//! any settings source, a cloud provider switch, a token or key variable.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use super::{oauth_status, read_json, summarise, AuthAdapter, ObservationFailed, Present};
use crate::harness::inventory::environment::DeviceEnvironment;
use crate::harness::inventory::model::{AuthFacts, AuthMethod, AuthStatus, Evidence};

pub struct ClaudeAuth;

pub static CLAUDE_AUTH: ClaudeAuth = ClaudeAuth;

/// What one settings file says about how a session authenticates.
#[derive(Debug, Default, Deserialize)]
struct Settings {
    #[serde(default, rename = "apiKeyHelper")]
    api_key_helper: Present,
    /// Variables the settings set for every session, by name.
    #[serde(default)]
    env: std::collections::BTreeMap<String, Present>,
}

/// The global config (`.claude.json`), for the one sign-in it can hold: an
/// API key saved by `/login`.
#[derive(Debug, Default, Deserialize)]
struct GlobalConfig {
    #[serde(default, rename = "primaryApiKey")]
    primary_api_key: Present,
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

/// Everything saved that a session would authenticate with.
struct Saved {
    settings: Vec<Settings>,
    oauth: Option<OauthCredentials>,
    saved_api_key: bool,
}

impl Saved {
    /// Every source read, or the first that could not be: a settings file
    /// Build cannot read could name anything.
    fn read(environment: &DeviceEnvironment) -> Result<Self, ObservationFailed> {
        let mut settings = Vec::new();
        for path in settings_paths(environment)? {
            settings.extend(read_json::<Settings>(&path)?);
        }
        let oauth = read_json::<CredentialsFile>(&credentials_path(environment))?
            .and_then(|file| file.oauth)
            .filter(|oauth| oauth.access.0 || oauth.refresh.0);
        let saved_api_key = read_json::<GlobalConfig>(&global_config_path(environment))?
            .is_some_and(|config| config.primary_api_key.0);
        Ok(Saved {
            settings,
            oauth,
            saved_api_key,
        })
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

    fn names_a_key_helper(&self) -> bool {
        self.settings
            .iter()
            .any(|settings| settings.api_key_helper.0)
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

/// `.claude.json`: in the config directory when `CLAUDE_CONFIG_DIR` moves
/// it, beside it in the home directory otherwise.
fn global_config_path(environment: &DeviceEnvironment) -> PathBuf {
    if environment.has("CLAUDE_CONFIG_DIR") {
        config_dir(environment).join(".claude.json")
    } else {
        environment.home().join(".claude.json")
    }
}

fn credentials_path(environment: &DeviceEnvironment) -> PathBuf {
    config_dir(environment).join(".credentials.json")
}

/// Every settings file a session started in the home directory reads: the
/// user's (in the config directory), the home directory's own project and
/// local `.claude` settings (the same files unless `CLAUDE_CONFIG_DIR` moves
/// the user's), and managed policy with its sorted drop-ins.
fn settings_paths(environment: &DeviceEnvironment) -> Result<Vec<PathBuf>, ObservationFailed> {
    let config = config_dir(environment);
    let project = environment.home().join(".claude");
    let mut paths = vec![
        config.join("settings.json"),
        config.join("settings.local.json"),
        config.join("cowork_settings.json"),
        project.join("settings.json"),
        project.join("settings.local.json"),
    ];
    let managed = environment.claude_managed_root();
    paths.push(managed.join("managed-settings.json"));
    paths.extend(drop_ins(&managed.join("managed-settings.d"))?);
    let mut seen = std::collections::BTreeSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    Ok(paths)
}

/// The `*.json` files of a drop-in directory, in name order.
fn drop_ins(directory: &std::path::Path) -> Result<Vec<PathBuf>, ObservationFailed> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => {
            return Err(ObservationFailed(
                "a managed settings directory could not be read",
            ))
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    Ok(paths)
}

impl AuthAdapter for ClaudeAuth {
    fn context(&self) -> &'static str {
        "claude"
    }

    fn watched(&self, environment: &DeviceEnvironment) -> Vec<PathBuf> {
        let mut watched = settings_paths(environment).unwrap_or_default();
        watched.push(environment.claude_managed_root().join("managed-settings.d"));
        watched.push(credentials_path(environment));
        watched.push(global_config_path(environment));
        watched
    }

    fn observe(
        &self,
        environment: &DeviceEnvironment,
        now: SystemTime,
    ) -> Result<AuthFacts, ObservationFailed> {
        let saved = Saved::read(environment)?;
        if saved.names_a_key_helper() {
            // A helper command supplies the key; Build never runs it.
            return Ok(AuthFacts::saved(
                AuthMethod::External,
                AuthStatus::Unknown,
                vec![Evidence::Settings],
            ));
        }
        if let Some(evidence) = EXTERNAL_PROVIDERS
            .iter()
            .find_map(|name| saved.sets(environment, name))
        {
            return Ok(AuthFacts::saved(
                AuthMethod::External,
                AuthStatus::Unknown,
                vec![evidence],
            ));
        }
        Ok(from_saved(environment, &saved, now))
    }
}

fn from_saved(environment: &DeviceEnvironment, saved: &Saved, now: SystemTime) -> AuthFacts {
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
    if saved.saved_api_key {
        note(
            AuthMethod::ApiKey,
            AuthStatus::SignedIn,
            Evidence::CredentialsFile,
        );
    }
    if let Some(oauth) = &saved.oauth {
        let status = oauth_status(oauth.expires_at, oauth.refresh.0, now);
        note(AuthMethod::Oauth, status, Evidence::CredentialsFile);
    }
    if parts.is_empty() {
        // Nothing saved: the absence is itself read from the credentials file.
        evidence.push(Evidence::CredentialsFile);
        if cfg!(target_os = "macos") {
            // There Claude Code keeps its sign-in in the Keychain, which no
            // file shows.
            return AuthFacts::saved(AuthMethod::Unknown, AuthStatus::Unknown, evidence);
        }
    }
    let (method, status) = summarise(&parts);
    AuthFacts::saved(method, status, evidence)
}
