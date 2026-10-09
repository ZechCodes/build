//! The inventory against fake CLIs and fixture credential files under
//! temporary homes: no real CLI, no network and no account is touched.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::*;
use crate::harness::installed::{CliProbe, CliReading};
use crate::isolation::test_fixture::write_executable;

/// A device: a temporary home with a `bin` on its `PATH`, the variables a
/// harness would spawn with, and clocks a test moves by hand.
struct Device {
    home: tempfile::TempDir,
    vars: Arc<Mutex<Vec<(String, String)>>>,
    clock: Arc<Mutex<Instant>>,
    wall: Arc<Mutex<SystemTime>>,
}

impl Device {
    fn new() -> Device {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("bin")).unwrap();
        let vars = vec![
            (
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin", home.path().join("bin").display()),
            ),
            ("HOME".to_string(), home.path().display().to_string()),
        ];
        Device {
            home,
            vars: Arc::new(Mutex::new(vars)),
            clock: Arc::new(Mutex::new(Instant::now())),
            wall: Arc::new(Mutex::new(SystemTime::now())),
        }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn set(&self, name: &str, value: &str) {
        self.vars.lock().unwrap().push((name.into(), value.into()));
    }

    fn write(&self, relative: &str, body: &str) -> PathBuf {
        let path = self.home().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    fn fifo(&self, relative: &str) -> PathBuf {
        let path = self.home().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path; mkfifo touches nothing else.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        path
    }

    /// A CLI that leaves `~/<marker>` if anything ever runs it.
    fn install_tripwire(&self, binary: &str, marker: &str) {
        self.install_tripwire_at(&self.home().join("bin").join(binary), marker);
    }

    fn install_tripwire_at(&self, path: &Path, marker: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_executable(path, &format!("#!/bin/sh\ntouch \"$HOME/{marker}\"\n"));
    }

    fn ran(&self, marker: &str) -> bool {
        self.home().join(marker).exists()
    }

    fn advance(&self, by: Duration) {
        *self.clock.lock().unwrap() += by;
        *self.wall.lock().unwrap() += by;
    }

    fn builder(&self) -> InventoryBuilder {
        let home = self.home().to_path_buf();
        let vars = Arc::clone(&self.vars);
        let clock = Arc::clone(&self.clock);
        let wall = Arc::clone(&self.wall);
        Inventory::builder()
            .environment(move || {
                DeviceEnvironment::new(home.clone(), vars.lock().unwrap().clone())
                    .with_claude_managed_root(home.clone())
            })
            .readings(Readings::answering_inline(&FIXED_VERSIONS))
            .running(RunningSessions::new())
            .clocks(
                move || *clock.lock().unwrap(),
                move || *wall.lock().unwrap(),
            )
    }

    fn inventory(&self) -> Arc<Inventory> {
        self.builder().build()
    }
}

/// Claude Code says 2.1.284; every other CLI says nothing readable.
struct FixedVersions;

static FIXED_VERSIONS: FixedVersions = FixedVersions;

impl CliProbe for FixedVersions {
    fn read(&self, binary: &str) -> CliReading {
        CliReading {
            version: (binary == "claude").then(|| semver::Version::new(2, 1, 284)),
            listed: None,
        }
    }
}

fn context<'a>(snapshot: &'a HarnessesSnapshot, id: &str) -> &'a AuthContextRow {
    snapshot
        .auth_contexts
        .iter()
        .find(|context| context.id == id)
        .unwrap()
}

fn harness(snapshot: &HarnessesSnapshot, id: AgentProvider) -> &HarnessRow {
    snapshot
        .harnesses
        .iter()
        .find(|harness| harness.id == id)
        .unwrap()
}

fn facts_of(snapshot: &HarnessesSnapshot, id: &str) -> (AuthMethod, AuthStatus, Health) {
    let row = context(snapshot, id);
    (row.facts.method, row.facts.status, row.health)
}

/// Signed in with Claude.ai: an access token that expires far ahead, beside
/// its refresh token.
const CLAUDE_SIGNED_IN: &str = r#"{"claudeAiOauth": {"accessToken": "sk-ant-oat-SECRET",
    "refreshToken": "sk-ant-ort-SECRET", "expiresAt": 99999999999999,
    "scopes": ["user:inference"]}}"#;

#[test]
fn every_harness_is_listed_linked_to_its_shared_credential_context() {
    let device = Device::new();
    let snapshot = device.inventory().snapshot();

    let ids: Vec<_> = snapshot
        .harnesses
        .iter()
        .map(|harness| harness.id)
        .collect();
    assert_eq!(ids, AgentProvider::ALL);
    let contexts: Vec<_> = snapshot
        .auth_contexts
        .iter()
        .map(|context| (context.id.as_str(), context.harnesses.clone()))
        .collect();
    assert_eq!(
        contexts,
        [
            (
                "claude",
                vec![AgentProvider::Claude, AgentProvider::ClaudeAdk]
            ),
            (
                "codex",
                vec![AgentProvider::Codex, AgentProvider::CodexAppServer]
            ),
            ("pi", vec![AgentProvider::Pi]),
        ]
    );
    for row in &snapshot.harnesses {
        assert_eq!(row.installation.state, InstallationState::Unknown);
        assert_eq!(row.installation.health, Health::Unobserved);
    }
    assert!(snapshot
        .auth_contexts
        .iter()
        .all(|context| context.facts == AuthFacts::unobserved()
            && context.supported_login_methods.is_empty()));
}

#[test]
fn claude_is_read_from_its_saved_credentials_and_never_run() {
    let device = Device::new();
    device.install_tripwire("claude", "claude-ran");
    device.write(".claude/.credentials.json", CLAUDE_SIGNED_IN);
    let inventory = device.inventory();
    inventory.sweep();

    let snapshot = inventory.snapshot();
    let claude = context(&snapshot, "claude");
    assert_eq!(
        (claude.facts.method, claude.facts.status),
        (AuthMethod::Oauth, AuthStatus::SignedIn)
    );
    assert_eq!(claude.facts.verification, Verification::SavedConfiguration);
    assert_eq!(claude.facts.evidence, [Evidence::CredentialsFile]);
    assert_eq!(claude.health, Health::Fresh);
    assert_eq!(claude.credential_generation, 1);
    assert!(!device.ran("claude-ran"));
    assert!(!serde_json::to_string(&snapshot).unwrap().contains("SECRET"));
}

#[test]
fn an_expired_claude_access_token_with_a_refresh_token_is_refresh_pending() {
    let device = Device::new();
    let credentials = |refresh: &str| {
        format!(
            r#"{{"claudeAiOauth": {{"accessToken": "sk-ant-oat-SECRET", {refresh}
               "expiresAt": 1000, "scopes": ["user:inference"]}}}}"#
        )
    };
    device.write(
        ".claude/.credentials.json",
        &credentials(r#""refreshToken": "sk-ant-ort-SECRET","#),
    );
    let inventory = device.inventory();
    inventory.sweep();
    let snapshot = inventory.snapshot();
    assert_eq!(
        context(&snapshot, "claude").facts.status,
        AuthStatus::RefreshPending
    );
    assert!(!serde_json::to_string(&snapshot).unwrap().contains("SECRET"));

    device.write(".claude/.credentials.json", &credentials(""));
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    let snapshot = inventory.snapshot();
    assert_eq!(
        context(&snapshot, "claude").facts.status,
        AuthStatus::Expired
    );
    assert_eq!(context(&snapshot, "claude").credential_generation, 2);
}

#[test]
fn a_claude_api_key_helper_in_any_settings_source_reads_as_external_and_never_runs() {
    let sources = [
        ".claude/settings.json",
        ".claude/settings.local.json",
        "managed-settings.json",
        "managed-settings.d/50-auth.json",
    ];
    for source in sources {
        let device = Device::new();
        device.install_tripwire("claude", "claude-ran");
        let helper = device.home().join("helper");
        device.install_tripwire_at(&helper, "helper-ran");
        device.write(".claude/.credentials.json", CLAUDE_SIGNED_IN);
        device.write(
            source,
            &format!(r#"{{"apiKeyHelper": "{}"}}"#, helper.display()),
        );
        let inventory = device.inventory();
        inventory.sweep();
        let snapshot = inventory.snapshot();
        assert_eq!(
            facts_of(&snapshot, "claude"),
            (AuthMethod::External, AuthStatus::Unknown, Health::Fresh),
            "{source}"
        );
        assert_eq!(
            context(&snapshot, "claude").facts.evidence,
            [Evidence::Settings]
        );
        assert!(
            !device.ran("claude-ran") && !device.ran("helper-ran"),
            "{source}"
        );
    }
}

/// #466: with `CLAUDE_CONFIG_DIR` elsewhere, the home directory's own
/// `.claude` settings are still the project settings of a session started
/// there.
#[test]
fn a_custom_claude_config_dir_still_reads_the_home_project_settings() {
    let device = Device::new();
    device.install_tripwire("claude", "claude-ran");
    device.set(
        "CLAUDE_CONFIG_DIR",
        &device.home().join("custom-claude").display().to_string(),
    );
    device.write("custom-claude/.credentials.json", CLAUDE_SIGNED_IN);
    device.write(
        ".claude/settings.local.json",
        r#"{"apiKeyHelper": "test-helper"}"#,
    );
    let inventory = device.inventory();
    inventory.sweep();
    assert_eq!(
        facts_of(&inventory.snapshot(), "claude").0,
        AuthMethod::External
    );
    assert!(!device.ran("claude-ran"));
}

/// #466: a policy helper is run by Claude Code's startup, which is why the
/// CLI is never started at all.
#[test]
fn a_claude_policy_helper_never_runs() {
    let device = Device::new();
    device.install_tripwire("claude", "claude-ran");
    let helper = device.home().join("policy-helper");
    device.install_tripwire_at(&helper, "policy-helper-ran");
    device.write(".claude/.credentials.json", CLAUDE_SIGNED_IN);
    device.write(
        "managed-settings.json",
        &format!(r#"{{"policyHelper": {{"path": "{}"}}}}"#, helper.display()),
    );
    let inventory = device.inventory();
    inventory.sweep();
    assert_eq!(
        facts_of(&inventory.snapshot(), "claude"),
        (AuthMethod::Oauth, AuthStatus::SignedIn, Health::Fresh)
    );
    assert!(!device.ran("claude-ran") && !device.ran("policy-helper-ran"));
}

/// #466: a settings file Build cannot read could name anything, so the
/// observation fails rather than reading past it.
#[test]
fn unreadable_claude_settings_fail_the_observation_closed() {
    let device = Device::new();
    device.install_tripwire("claude", "claude-ran");
    device.write(".claude/.credentials.json", CLAUDE_SIGNED_IN);
    let inventory = device.inventory();
    inventory.sweep();

    let oversized =
        serde_json::json!({"apiKeyHelper": "test-helper", "padding": "x".repeat(1024 * 1024)});
    device.write(".claude/settings.json", &oversized.to_string());
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    assert_eq!(
        facts_of(&inventory.snapshot(), "claude"),
        (AuthMethod::Oauth, AuthStatus::SignedIn, Health::Stale)
    );
    assert!(!device.ran("claude-ran"));
}

#[test]
fn claude_without_saved_credentials_reads_the_environment_by_name_only() {
    let device = Device::new();
    device.set("ANTHROPIC_API_KEY", "sk-ant-api-SECRET");
    let inventory = device.inventory();
    inventory.sweep();
    let snapshot = inventory.snapshot();
    let claude = context(&snapshot, "claude");
    assert_eq!(
        (claude.facts.method, claude.facts.status),
        (AuthMethod::ApiKey, AuthStatus::SignedIn)
    );
    assert_eq!(claude.facts.evidence, [Evidence::Environment]);
    assert_eq!(
        harness(&snapshot, AgentProvider::Claude).installation.state,
        InstallationState::NotInstalled
    );
    assert!(!serde_json::to_string(&snapshot).unwrap().contains("SECRET"));
}

#[test]
fn codex_is_read_from_its_saved_metadata_and_never_run() {
    let device = Device::new();
    device.install_tripwire("codex", "codex-ran");
    let inventory = device.inventory();
    inventory.sweep();
    assert_eq!(
        facts_of(&inventory.snapshot(), "codex"),
        (AuthMethod::None, AuthStatus::NotSignedIn, Health::Fresh)
    );

    device.write(
        ".codex/auth.json",
        r#"{"auth_mode": "chatgpt", "OPENAI_API_KEY": null,
            "tokens": {"id_token": "eyJ.SECRET", "access_token": "eyJ.SECRET",
                       "refresh_token": "rt-SECRET", "account_id": "acct-SECRET"},
            "last_refresh": "2026-10-01T00:00:00Z"}"#,
    );
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    let snapshot = inventory.snapshot();
    let codex = context(&snapshot, "codex");
    assert_eq!(
        (codex.facts.method, codex.facts.status),
        (AuthMethod::Oauth, AuthStatus::SignedIn)
    );
    assert_eq!(codex.facts.evidence, [Evidence::CredentialsFile]);
    assert_eq!(
        codex.harnesses,
        [AgentProvider::Codex, AgentProvider::CodexAppServer]
    );

    device.write(
        ".codex/auth.json",
        r#"{"auth_mode": "apikey", "OPENAI_API_KEY": "sk-proj-SECRET"}"#,
    );
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    let snapshot = inventory.snapshot();
    assert_eq!(context(&snapshot, "codex").facts.method, AuthMethod::ApiKey);

    assert!(!device.ran("codex-ran"));
    assert!(!serde_json::to_string(&snapshot).unwrap().contains("SECRET"));
}

/// #466: valid TOML, comments and literal strings included.
#[test]
fn codex_credentials_in_the_keyring_are_unknown_rather_than_absent() {
    for config in [
        "model = \"gpt-6\"\ncli_auth_credentials_store = \"keyring\"\n",
        "cli_auth_credentials_store = \"keyring\" # use system storage\n",
        "cli_auth_credentials_store = 'auto'\n[profiles.work]\nmodel = 'x'\n",
    ] {
        let device = Device::new();
        device.write(".codex/config.toml", config);
        let inventory = device.inventory();
        inventory.sweep();
        assert_eq!(
            facts_of(&inventory.snapshot(), "codex").1,
            AuthStatus::Unknown,
            "{config}"
        );
    }
    // A table's own key of that name is not the top-level setting.
    let device = Device::new();
    device.write(
        ".codex/config.toml",
        "[profiles.work]\ncli_auth_credentials_store = \"keyring\"\n",
    );
    let inventory = device.inventory();
    inventory.sweep();
    assert_eq!(
        facts_of(&inventory.snapshot(), "codex").1,
        AuthStatus::NotSignedIn
    );
}

/// #466: a credential file that breaks keeps what it said before, stale,
/// rather than reading as freshly signed out.
#[test]
fn malformed_credential_files_keep_the_prior_facts_stale() {
    let device = Device::new();
    device.write(".claude/.credentials.json", CLAUDE_SIGNED_IN);
    device.write(
        ".codex/auth.json",
        r#"{"auth_mode": "apikey", "OPENAI_API_KEY": "sk-SECRET"}"#,
    );
    device.write(
        ".pi/agent/auth.json",
        r#"{"openai": {"type": "api_key", "key": "sk-SECRET"}}"#,
    );
    let inventory = device.inventory();
    inventory.sweep();
    for id in ["claude", "codex", "pi"] {
        assert_eq!(
            facts_of(&inventory.snapshot(), id).1,
            AuthStatus::SignedIn,
            "{id}"
        );
    }

    device.write(".claude/.credentials.json", "{");
    device.write(".codex/auth.json", "{");
    device.write(".pi/agent/auth.json", "{");
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    let snapshot = inventory.snapshot();
    for id in ["claude", "codex", "pi"] {
        let (_, status, health) = facts_of(&snapshot, id);
        assert_eq!(
            (status, health),
            (AuthStatus::SignedIn, Health::Stale),
            "{id}"
        );
    }
}

/// #466: a FIFO (or any non-regular file) where a credential or config file
/// belongs is refused without blocking the sweep.
#[test]
fn a_fifo_in_place_of_a_metadata_file_never_blocks_the_sweep() {
    for (path, id) in [
        (".codex/config.toml", "codex"),
        (".codex/auth.json", "codex"),
        (".claude/.credentials.json", "claude"),
        (".claude/settings.json", "claude"),
        (".pi/agent/auth.json", "pi"),
    ] {
        let device = Device::new();
        let fifo = device.fifo(path);
        let inventory = device.inventory();
        let (done, finished) = std::sync::mpsc::channel();
        let sweeping = Arc::clone(&inventory);
        let worker = std::thread::spawn(move || {
            sweeping.sweep();
            done.send(()).unwrap();
        });
        if finished.recv_timeout(Duration::from_secs(10)).is_err() {
            // Release the blocked reader before failing, so no thread hangs.
            drop(std::fs::OpenOptions::new().write(true).open(&fifo));
            worker.join().unwrap();
            panic!("{path}: a FIFO blocked the sweep");
        }
        worker.join().unwrap();
        assert_ne!(
            facts_of(&inventory.snapshot(), id).2,
            Health::Fresh,
            "{path}: a refused file is no observation"
        );
    }
}

#[test]
fn pi_lists_each_provider_without_resolving_keys_or_running_commands() {
    let device = Device::new();
    device.install_tripwire("pi", "pi-ran");
    let command = device.home().join("key-command");
    device.install_tripwire_at(&command, "key-command-ran");
    device.write(
        ".pi/agent/auth.json",
        &format!(
            r#"{{
              "anthropic": {{"type": "oauth", "refresh": "rt-SECRET", "access": "at-SECRET", "expires": 1000}},
              "openai": {{"type": "api_key", "key": "sk-SECRET"}},
              "deepseek": {{"type": "api_key", "key": "!{} SECRET"}},
              "google": {{"type": "api_key", "key": "$GEMINI_SECRET"}},
              "Bad Provider<script>": {{"type": "api_key", "key": "x"}}
            }}"#,
            command.display()
        ),
    );
    device.set("GROQ_API_KEY", "gsk-SECRET");
    device.set("OPENAI_API_KEY", "sk-env-SECRET");
    let inventory = device.inventory();
    inventory.sweep();

    let snapshot = inventory.snapshot();
    let pi = context(&snapshot, "pi");
    let providers: Vec<_> = pi
        .facts
        .providers
        .iter()
        .map(|provider| {
            (
                provider.id.as_str(),
                provider.method,
                provider.status,
                provider.evidence.clone(),
            )
        })
        .collect();
    assert_eq!(
        providers,
        [
            (
                "anthropic",
                AuthMethod::Oauth,
                AuthStatus::RefreshPending,
                vec![Evidence::CredentialsFile]
            ),
            (
                "deepseek",
                AuthMethod::External,
                AuthStatus::Unknown,
                vec![Evidence::CredentialsFile]
            ),
            (
                "google",
                AuthMethod::ApiKey,
                AuthStatus::Unknown,
                vec![Evidence::CredentialsFile]
            ),
            (
                "groq",
                AuthMethod::ApiKey,
                AuthStatus::SignedIn,
                vec![Evidence::Environment]
            ),
            (
                "openai",
                AuthMethod::ApiKey,
                AuthStatus::SignedIn,
                vec![Evidence::CredentialsFile]
            ),
        ]
    );
    assert_eq!(
        (pi.facts.method, pi.facts.status),
        (AuthMethod::Mixed, AuthStatus::SignedIn)
    );
    assert_eq!(
        pi.facts.evidence,
        [Evidence::CredentialsFile, Evidence::Environment]
    );
    assert!(!device.ran("key-command-ran"));
    assert!(!device.ran("pi-ran"));
    let wire = serde_json::to_string(&snapshot).unwrap();
    assert!(
        !wire.contains("SECRET") && !wire.contains("script"),
        "{wire}"
    );
}

#[test]
fn installs_removals_and_retargeted_links_are_seen_and_check_sign_in_again() {
    let device = Device::new();
    let inventory = device.inventory();
    inventory.sweep();
    let snapshot = inventory.snapshot();
    assert_eq!(
        harness(&snapshot, AgentProvider::ClaudeAdk)
            .installation
            .state,
        InstallationState::NotInstalled
    );
    assert_eq!(
        harness(&snapshot, AgentProvider::ClaudeAdk)
            .version
            .installed,
        None
    );
    assert_eq!(inventory.checks_of("claude"), 1);

    let first = device.home().join("versions/claude-a");
    device.install_tripwire_at(&first, "claude-ran");
    std::os::unix::fs::symlink(&first, device.home().join("bin/claude")).unwrap();
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    let snapshot = inventory.snapshot();
    let adk = harness(&snapshot, AgentProvider::ClaudeAdk);
    assert_eq!(adk.installation.state, InstallationState::Installed);
    assert_eq!(adk.version.installed.as_deref(), Some("2.1.284"));
    assert_eq!(
        inventory.checks_of("claude"),
        2,
        "the install was checked at once"
    );

    let second = device.home().join("versions/claude-b");
    device.install_tripwire_at(&second, "claude-ran");
    std::fs::remove_file(device.home().join("bin/claude")).unwrap();
    std::os::unix::fs::symlink(&second, device.home().join("bin/claude")).unwrap();
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    assert_eq!(
        inventory.checks_of("claude"),
        3,
        "the retargeted link was checked well inside the minute"
    );

    std::fs::remove_file(device.home().join("bin/claude")).unwrap();
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    let snapshot = inventory.snapshot();
    assert_eq!(
        harness(&snapshot, AgentProvider::Claude).installation.state,
        InstallationState::NotInstalled
    );
    assert_eq!(
        harness(&snapshot, AgentProvider::Claude).version.installed,
        None
    );
    assert!(!device.ran("claude-ran"));
}

#[test]
fn installed_and_running_versions_are_reported_apart_and_unknown_stays_unknown() {
    let device = Device::new();
    device.install_tripwire("claude", "claude-ran");
    device.install_tripwire("codex", "codex-ran");
    let running = RunningSessions::new();
    let inventory = device.builder().running(Arc::clone(&running)).build();
    let mut changes = inventory.changes();
    inventory.sweep();
    changes.mark_unchanged();
    let session = running.report(AgentProvider::ClaudeAdk, "2.1.280".into());
    let other = running.report(AgentProvider::ClaudeAdk, "2.1.280".into());
    inventory.sweep();
    assert!(changes.has_changed().unwrap());

    let snapshot = inventory.snapshot();
    let adk = &harness(&snapshot, AgentProvider::ClaudeAdk).version;
    assert_eq!(adk.installed.as_deref(), Some("2.1.284"));
    assert!(adk.running.reported);
    assert_eq!(
        adk.running.versions,
        [RunningVersion {
            version: "2.1.280".into(),
            sessions: 2
        }]
    );
    // The TUI says nothing about what it runs: not reported, not borrowed.
    let tui = &harness(&snapshot, AgentProvider::Claude).version;
    assert_eq!(tui.installed.as_deref(), Some("2.1.284"));
    assert!(!tui.running.reported && tui.running.versions.is_empty());
    // Codex is installed and said no readable version.
    let codex = harness(&snapshot, AgentProvider::Codex);
    assert_eq!(codex.installation.state, InstallationState::Installed);
    assert_eq!(codex.version.installed, None);

    drop(session);
    drop(other);
    changes.mark_unchanged();
    inventory.sweep();
    assert!(changes.has_changed().unwrap());
    let snapshot = inventory.snapshot();
    assert!(harness(&snapshot, AgentProvider::ClaudeAdk)
        .version
        .running
        .versions
        .is_empty());
}

#[test]
fn a_sign_in_check_runs_at_most_once_a_minute_and_backs_off_after_failures() {
    let device = Device::new();
    device.write(".claude/.credentials.json", CLAUDE_SIGNED_IN);
    let inventory = device.inventory();
    inventory.sweep();
    device.advance(SWEEP_INTERVAL);
    inventory.sweep();
    assert_eq!(inventory.checks_of("claude"), 1);
    device.advance(AUTH_CHECK_INTERVAL);
    inventory.sweep();
    assert_eq!(inventory.checks_of("claude"), 2);

    // The file breaks: what it said before stands, stale. Its change is
    // checked at once; the failures after it back off.
    device.write(".claude/.credentials.json", "{");
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    assert_eq!(inventory.checks_of("claude"), 3);
    assert_eq!(
        facts_of(&inventory.snapshot(), "claude"),
        (AuthMethod::Oauth, AuthStatus::SignedIn, Health::Stale)
    );
    // The failed check waits a minute, the next one two.
    device.advance(AUTH_CHECK_INTERVAL);
    inventory.sweep();
    assert_eq!(inventory.checks_of("claude"), 4);
    device.advance(AUTH_CHECK_INTERVAL);
    inventory.sweep();
    assert_eq!(inventory.checks_of("claude"), 4);
    device.advance(AUTH_CHECK_INTERVAL);
    inventory.sweep();
    assert_eq!(inventory.checks_of("claude"), 5);
    assert_eq!(backoff(20), MAX_FAILURE_BACKOFF);
}

#[test]
fn a_refresh_answers_at_once_and_completes_with_the_next_sweep() {
    let device = Device::new();
    let inventory = device.inventory();
    inventory.sweep();
    let before = inventory.snapshot();

    let receipt = inventory.request_refresh();
    assert_eq!(
        receipt,
        RefreshReceipt {
            request: 1,
            revision: before.revision
        }
    );
    assert_eq!(
        inventory.checks_of("claude"),
        1,
        "a refresh observes nothing itself"
    );

    // Inside the floor the check waits, and the refresh stays open.
    inventory.sweep();
    assert_eq!(inventory.snapshot().refresh.completed, 0);
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    let after = inventory.snapshot();
    assert_eq!(inventory.checks_of("claude"), 2);
    assert_eq!(
        after.refresh,
        RefreshProgress {
            requested: 1,
            completed: 1
        }
    );
    assert!(after.revision > before.revision);
}

#[test]
fn an_older_observation_never_overwrites_a_newer_one() {
    let now = Instant::now();
    let mut context = ContextState::new(&CLAUDE_AUTH);
    let older = context.claim(&[], now).unwrap();
    // A second check is claimed only once the first is in.
    assert_eq!(context.claim(&[], now), None);
    context.claimed += 1;
    let newer = context.claimed;
    let signed_out = AuthFacts::saved(AuthMethod::None, AuthStatus::NotSignedIn, vec![]);
    let signed_in = AuthFacts::saved(AuthMethod::Oauth, AuthStatus::SignedIn, vec![]);
    assert!(context.record(newer, Ok(signed_in.clone()), vec![], now, SystemTime::now()));
    assert!(!context.record(older, Ok(signed_out), vec![], now, SystemTime::now()));
    assert_eq!(context.facts, signed_in);
}

/// No client is connected and nothing asks: the service's own thread sweeps,
/// and the change it announces is the event waited on.
#[test]
fn the_sweep_runs_with_no_client_connected() {
    let device = Device::new();
    device.write(".claude/.credentials.json", CLAUDE_SIGNED_IN);
    let inventory = device.inventory();
    let mut changes = inventory.changes();
    inventory.start();
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(30), changes.changed())
                .await
                .expect("the first sweep announces itself")
                .unwrap();
        });
    assert_eq!(
        facts_of(&inventory.snapshot(), "claude"),
        (AuthMethod::Oauth, AuthStatus::SignedIn, Health::Fresh)
    );
}

#[test]
fn a_restart_restores_nonsecret_observations_as_stale() {
    let device = Device::new();
    device.write(".claude/.credentials.json", CLAUDE_SIGNED_IN);
    device.write(
        ".codex/auth.json",
        r#"{"auth_mode": "chatgpt", "OPENAI_API_KEY": "sk-SECRET",
            "tokens": {"access_token": "at-SECRET", "refresh_token": "rt-SECRET", "id_token": "id-SECRET"}}"#,
    );
    device.write(
        ".pi/agent/auth.json",
        r#"{"anthropic": {"type": "oauth", "access": "at-SECRET", "refresh": "rt-SECRET", "expires": 99999999999999},
            "openai": {"type": "api_key", "key": "sk-SECRET"}}"#,
    );
    let saved = device.home().join("state/harness-inventory.json");
    std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
    let inventory = device.builder().persist_to(saved.clone()).build();
    inventory.sweep();
    let before = inventory.snapshot();

    let written = std::fs::read_to_string(&saved).unwrap();
    let push = crate::changes::harnesses_changed_payload(before.revision).to_string();
    for text in [&written, &serde_json::to_string(&before).unwrap(), &push] {
        assert!(!text.contains("SECRET"), "{text}");
    }
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let restarted = device.builder().persist_to(saved).build().snapshot();
    let claude = context(&restarted, "claude");
    assert_eq!(claude.facts, context(&before, "claude").facts);
    assert_eq!(claude.health, Health::Stale);
    assert_eq!(
        claude.credential_generation,
        context(&before, "claude").credential_generation
    );
    assert_eq!(
        harness(&restarted, AgentProvider::Claude)
            .installation
            .health,
        Health::Stale
    );
    assert!(restarted.revision > before.revision);
}

/// #466 round 2: an API key saved by `/login` lives as `primaryApiKey` in
/// Claude Code's global config, not in the credentials file.
#[test]
fn a_claude_api_key_saved_by_login_reads_as_signed_in() {
    let device = Device::new();
    device.write(
        ".claude.json",
        r#"{"primaryApiKey": "sk-ant-api-SECRET", "numStartups": 3}"#,
    );
    let inventory = device.inventory();
    inventory.sweep();
    let snapshot = inventory.snapshot();
    assert_eq!(
        facts_of(&snapshot, "claude"),
        (AuthMethod::ApiKey, AuthStatus::SignedIn, Health::Fresh)
    );
    assert_eq!(
        context(&snapshot, "claude").facts.evidence,
        [Evidence::CredentialsFile]
    );
    assert!(!serde_json::to_string(&snapshot).unwrap().contains("SECRET"));

    // With the config directory moved, the global config moves with it.
    let moved = Device::new();
    moved.set(
        "CLAUDE_CONFIG_DIR",
        &moved.home().join("custom-claude").display().to_string(),
    );
    moved.write(
        "custom-claude/.claude.json",
        r#"{"primaryApiKey": "sk-ant-api-SECRET"}"#,
    );
    let inventory = moved.inventory();
    inventory.sweep();
    assert_eq!(
        facts_of(&inventory.snapshot(), "claude").0,
        AuthMethod::ApiKey
    );
}

/// #466 round 2: an empty or whitespace-only value is no credential, in a
/// file, a settings `env` or the environment.
#[test]
fn empty_or_blank_credential_values_count_as_absent() {
    let device = Device::new();
    device.write(
        ".claude/settings.json",
        r#"{"env": {"ANTHROPIC_API_KEY": "", "CLAUDE_CODE_OAUTH_TOKEN": "   "}}"#,
    );
    device.write(".claude.json", r#"{"primaryApiKey": " "}"#);
    device.write(
        ".claude/.credentials.json",
        r#"{"claudeAiOauth": {"accessToken": "", "refreshToken": "  ", "expiresAt": 99999999999999}}"#,
    );
    device.write(
        ".codex/auth.json",
        r#"{"OPENAI_API_KEY": " ", "tokens": {"access_token": "", "refresh_token": ""}}"#,
    );
    device.write(
        ".pi/agent/auth.json",
        r#"{"openai": {"type": "api_key", "key": "   "},
            "anthropic": {"type": "oauth", "access": "", "refresh": " ", "expires": 99999999999999}}"#,
    );
    device.set("CODEX_API_KEY", "  ");
    device.set("GROQ_API_KEY", " ");
    let inventory = device.inventory();
    inventory.sweep();
    let snapshot = inventory.snapshot();
    for id in ["claude", "codex", "pi"] {
        assert_eq!(
            facts_of(&snapshot, id),
            (AuthMethod::None, AuthStatus::NotSignedIn, Health::Fresh),
            "{id}"
        );
    }
    assert!(context(&snapshot, "pi").facts.providers.is_empty());
}
