//! What the harness inventory says, as `harnesses.list` carries it.
//!
//! Three facts per harness, kept apart because each changes on its own: is
//! the CLI installed, which version is installed (and, separately, which
//! versions its running sessions reported), and how its credential context
//! is signed in. Nothing here is a secret: methods and statuses are enums,
//! evidence names a kind of source, and a Pi provider is its id.

use serde::{Deserialize, Serialize};

use crate::models::AgentProvider;

/// The whole inventory at one revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessesSnapshot {
    /// Rises with every change to anything below, across bridge restarts: a
    /// client keeps the highest it has painted and ignores an older answer.
    pub revision: u64,
    pub refresh: RefreshProgress,
    /// Every harness, in `AgentProvider::ALL` order.
    pub harnesses: Vec<HarnessRow>,
    /// Every credential context, each naming the harnesses that share it.
    pub auth_contexts: Vec<AuthContextRow>,
}

/// How far explicit refreshes have got: a `harnesses.refresh` receipt's
/// `request` is done once `completed` reaches it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshProgress {
    pub requested: u64,
    pub completed: u64,
}

/// What `harnesses.refresh` answers at once: the request to watch for, and
/// the revision it was asked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshReceipt {
    pub request: u64,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessRow {
    pub id: AgentProvider,
    pub label: String,
    /// What a person calls the CLI: "Claude Code" for both its carriers.
    pub cli_name: String,
    /// The [`AuthContextRow::id`] this harness signs in through.
    pub auth_context: String,
    pub installation: Installation,
    pub version: VersionFacts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installation {
    pub state: InstallationState,
    pub health: Health,
    /// When it was last looked for, in Unix milliseconds.
    pub checked_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationState {
    Installed,
    NotInstalled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionFacts {
    /// What the installed CLI says it is, or `null` where it has not said
    /// (not installed, not read yet, or unreadable).
    pub installed: Option<String>,
    /// What the harness's live sessions said they run. A session may predate
    /// an update, so this is never the installed version copied.
    pub running: RunningVersions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningVersions {
    /// Whether this harness's sessions report a version at all. The TUIs do
    /// not: an empty list then means "not reported", not "none running".
    pub reported: bool,
    pub versions: Vec<RunningVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningVersion {
    pub version: String,
    /// How many live sessions reported it.
    pub sessions: u32,
}

/// One credential context: the effective home, config and provider
/// environment a harness spawns with. Harnesses sharing a CLI share one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthContextRow {
    pub id: String,
    pub harnesses: Vec<AgentProvider>,
    #[serde(flatten)]
    pub facts: AuthFacts,
    pub health: Health,
    /// When it was last observed successfully, in Unix milliseconds.
    pub checked_at_ms: Option<u64>,
    /// Rises each time the observed credential changes: its method, its
    /// status or the files it is kept in.
    pub credential_generation: u64,
    /// The sign-in methods Build can start for it. None yet (#434).
    pub supported_login_methods: Vec<String>,
}

/// What one observation found about a credential context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthFacts {
    pub method: AuthMethod,
    pub status: AuthStatus,
    pub verification: Verification,
    pub evidence: Vec<Evidence>,
    /// Pi's providers, each signed in on its own. Empty elsewhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderAuth>,
}

impl AuthFacts {
    /// Before anything has been observed.
    pub fn unobserved() -> Self {
        AuthFacts {
            method: AuthMethod::Unknown,
            status: AuthStatus::Unknown,
            verification: Verification::NotObserved,
            evidence: Vec::new(),
            providers: Vec::new(),
        }
    }

    /// Read from what is saved on this machine.
    pub fn saved(method: AuthMethod, status: AuthStatus, evidence: Vec<Evidence>) -> Self {
        AuthFacts {
            method,
            status,
            verification: Verification::SavedConfiguration,
            evidence,
            providers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    Oauth,
    LongLivedToken,
    ApiKey,
    /// A helper command, a cloud provider or an externally managed token:
    /// managed on this device, outside the CLI's own sign-in.
    External,
    Mixed,
    None,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStatus {
    SignedIn,
    NotSignedIn,
    Expired,
    Invalid,
    /// The access credential expired with a refresh credential beside it:
    /// the CLI refreshes it on next use, so nothing is wrong yet.
    RefreshPending,
    Unknown,
    NotRequired,
}

/// How far a status was checked. Every passive observation reads saved
/// configuration; none asks the provider, so "signed in" here never means a
/// current network check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    SavedConfiguration,
    NotObserved,
}

/// The kind of source a status was read from, never the source itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// The CLI's own status command, read through an allowlist.
    CliStatus,
    /// The CLI's saved credentials file: expiry and which credentials it
    /// holds, never their values.
    CredentialsFile,
    /// A variable in the environment the harness spawns with, by name.
    Environment,
    /// The CLI's settings, for helpers and providers it is configured with.
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAuth {
    pub id: String,
    pub method: AuthMethod,
    pub status: AuthStatus,
    pub evidence: Vec<Evidence>,
}

/// Whether a fact is current. A failed observation keeps the facts it could
/// not replace and marks them stale; so does a bridge restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Fresh,
    Stale,
    Unobserved,
}
