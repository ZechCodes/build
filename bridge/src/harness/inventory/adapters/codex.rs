//! Codex's credential context, shared by its TUI and its app server.
//!
//! Metadata only: Codex is never run to observe it. `codex login status`
//! prints part of an API key, and an app server started for `account/read`
//! loads the account through the same manager that refreshes tokens, which
//! the pinned 0.160.0 cannot be shown to skip. So the saved `auth.json` under
//! `CODEX_HOME` is read for its mode and which credentials it holds, and
//! `config.toml` for whether credentials live in the keyring instead.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use super::{read_json, read_metadata, AuthAdapter, ObservationFailed, Present};
use crate::harness::inventory::environment::DeviceEnvironment;
use crate::harness::inventory::model::{AuthFacts, AuthMethod, AuthStatus, Evidence};

pub struct CodexAuth;

pub static CODEX_AUTH: CodexAuth = CodexAuth;

#[derive(Debug, Default, Deserialize)]
struct AuthFile {
    #[serde(default)]
    auth_mode: Option<String>,
    #[serde(default, rename = "OPENAI_API_KEY")]
    api_key: Present,
    #[serde(default)]
    tokens: Option<Tokens>,
}

#[derive(Debug, Default, Deserialize)]
struct Tokens {
    #[serde(default)]
    access_token: Present,
    #[serde(default)]
    refresh_token: Present,
}

/// How a saved `auth.json` says Codex signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    ApiKey,
    ChatGpt,
    ExternalTokens,
    Unstated,
}

impl Mode {
    fn of(said: Option<&str>) -> Mode {
        let normalised = said.map(|mode| mode.replace('_', "").to_ascii_lowercase());
        match normalised.as_deref() {
            Some("apikey") => Mode::ApiKey,
            Some("chatgpt") => Mode::ChatGpt,
            Some("chatgptauthtokens") => Mode::ExternalTokens,
            _ => Mode::Unstated,
        }
    }
}

fn codex_home(environment: &DeviceEnvironment) -> PathBuf {
    environment.dir_or_home("CODEX_HOME", ".codex")
}

impl AuthAdapter for CodexAuth {
    fn context(&self) -> &'static str {
        "codex"
    }

    fn watched(&self, environment: &DeviceEnvironment) -> Vec<PathBuf> {
        let home = codex_home(environment);
        vec![home.join("auth.json"), home.join("config.toml")]
    }

    fn observe(
        &self,
        environment: &DeviceEnvironment,
        _now: SystemTime,
    ) -> Result<AuthFacts, ObservationFailed> {
        let home = codex_home(environment);
        if let Some(file) = read_json::<AuthFile>(&home.join("auth.json"))? {
            return Ok(from_file(&file));
        }
        if environment.has("CODEX_API_KEY") {
            return Ok(AuthFacts::saved(
                AuthMethod::ApiKey,
                AuthStatus::SignedIn,
                vec![Evidence::Environment],
            ));
        }
        if stores_in_keyring(&home.join("config.toml"))? {
            return Ok(AuthFacts::saved(
                AuthMethod::Unknown,
                AuthStatus::Unknown,
                vec![Evidence::Settings],
            ));
        }
        Ok(AuthFacts::saved(
            AuthMethod::None,
            AuthStatus::NotSignedIn,
            vec![Evidence::CredentialsFile],
        ))
    }
}

fn from_file(file: &AuthFile) -> AuthFacts {
    let tokens = file
        .tokens
        .as_ref()
        .is_some_and(|tokens| tokens.access_token.0 || tokens.refresh_token.0);
    let method = match (Mode::of(file.auth_mode.as_deref()), tokens, file.api_key.0) {
        // A mode names how tokens arrive; only a token is a sign-in.
        (Mode::ExternalTokens, true, _) => AuthMethod::External,
        (Mode::ExternalTokens, false, _) => AuthMethod::None,
        (Mode::ApiKey, _, true) | (Mode::Unstated, false, true) => AuthMethod::ApiKey,
        (Mode::ChatGpt, true, _) | (Mode::Unstated, true, false) => AuthMethod::Oauth,
        (Mode::Unstated, true, true) => AuthMethod::Mixed,
        _ => AuthMethod::None,
    };
    let status = match method {
        AuthMethod::None => AuthStatus::NotSignedIn,
        _ => AuthStatus::SignedIn,
    };
    AuthFacts::saved(method, status, vec![Evidence::CredentialsFile])
}

/// The one setting of `config.toml` read: where Codex keeps credentials.
#[derive(Debug, Default, Deserialize)]
struct Config {
    #[serde(default)]
    cli_auth_credentials_store: Option<String>,
}

/// Whether `config.toml` keeps credentials in the OS keyring, where no file
/// says anything: its top-level `cli_auth_credentials_store`, parsed as the
/// TOML it is.
fn stores_in_keyring(config: &Path) -> Result<bool, ObservationFailed> {
    let Some(bytes) = read_metadata(config)? else {
        return Ok(false);
    };
    let text =
        std::str::from_utf8(&bytes).map_err(|_| ObservationFailed("config.toml is not UTF-8"))?;
    let config: Config =
        toml::from_str(text).map_err(|_| ObservationFailed("config.toml is malformed"))?;
    Ok(matches!(
        config.cli_auth_credentials_store.as_deref(),
        Some("keyring" | "auto")
    ))
}
