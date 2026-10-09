//! The inventory against fake CLIs under temporary homes: no real CLI, no
//! network and no account is touched.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::*;
use crate::harness::installed::{CliProbe, CliReading};
use crate::isolation::test_fixture::write_executable;

/// A device: a temporary home with a `bin` on its `PATH`, and the variables
/// a harness would spawn with.
struct Device {
    home: tempfile::TempDir,
    vars: Arc<Mutex<Vec<(String, String)>>>,
    clock: Arc<Mutex<Instant>>,
}

impl Device {
    fn new() -> Device {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("bin")).unwrap();
        let vars = vec![
            // The device's own bin first; the system's for the fakes' `cat`.
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

    /// A fake `claude` that logs how it was run and answers `auth status`
    /// with `~/claude-status.json`.
    fn install_claude(&self) {
        self.install_claude_at(&self.home().join("bin/claude"));
    }

    fn install_claude_at(&self, path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_executable(
            path,
            "#!/bin/sh\n\
             echo \"$@\" >> \"$HOME/claude-calls\"\n\
             env > \"$HOME/claude-env\"\n\
             pwd > \"$HOME/claude-cwd\"\n\
             cat \"$HOME/claude-status.json\"\n",
        );
    }

    /// A CLI that leaves `~/<marker>` if anything ever runs it.
    fn install_tripwire(&self, binary: &str, marker: &str) {
        write_executable(
            &self.home().join("bin").join(binary),
            &format!("#!/bin/sh\ntouch \"$HOME/{marker}\"\n"),
        );
    }

    fn claude_calls(&self) -> usize {
        std::fs::read_to_string(self.home().join("claude-calls"))
            .map(|calls| calls.lines().count())
            .unwrap_or(0)
    }

    fn advance(&self, by: Duration) {
        *self.clock.lock().unwrap() += by;
    }

    fn builder(&self) -> InventoryBuilder {
        let home = self.home().to_path_buf();
        let vars = Arc::clone(&self.vars);
        let managed = home.join("managed-settings.json");
        let clock = Arc::clone(&self.clock);
        Inventory::builder()
            .environment(move || {
                DeviceEnvironment::new(home.clone(), vars.lock().unwrap().clone())
                    .with_claude_managed_settings(vec![managed.clone()])
            })
            .readings(Readings::answering_inline(&FIXED_VERSIONS))
            .running(RunningSessions::new())
            .clocks(move || *clock.lock().unwrap(), SystemTime::now)
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

const SIGNED_IN_WITH_CLAUDE_AI: &str = r#"{
  "loggedIn": true,
  "authMethod": "claude.ai",
  "apiProvider": "firstParty",
  "email": "person@example.com",
  "orgId": "org-secret-id",
  "orgName": "Example Org",
  "configDirectory": "/home/person/.claude"
}"#;

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
fn claude_status_is_read_through_its_allowlist_and_nothing_else_is_kept() {
    let device = Device::new();
    device.install_claude();
    device.write("claude-status.json", SIGNED_IN_WITH_CLAUDE_AI);
    let inventory = device.inventory();
    inventory.sweep();

    let snapshot = inventory.snapshot();
    let claude = context(&snapshot, "claude");
    assert_eq!(claude.facts.method, AuthMethod::Oauth);
    assert_eq!(claude.facts.status, AuthStatus::SignedIn);
    assert_eq!(claude.facts.verification, Verification::SavedConfiguration);
    assert_eq!(claude.facts.evidence, [Evidence::CliStatus]);
    assert_eq!(claude.health, Health::Fresh);
    assert_eq!(claude.credential_generation, 1);
    let wire = serde_json::to_string(&snapshot).unwrap();
    for private in [
        "person@example.com",
        "org-secret-id",
        "Example Org",
        "/home/person",
    ] {
        assert!(
            !wire.contains(private),
            "{private} reached the wire: {wire}"
        );
    }
    // One CLI, two carriers: asked once.
    assert_eq!(device.claude_calls(), 1);
}

#[test]
fn the_status_probe_runs_fixed_argv_from_home_without_agent_or_bridge_identity() {
    let device = Device::new();
    device.install_claude();
    device.write("claude-status.json", SIGNED_IN_WITH_CLAUDE_AI);
    for name in [
        "BRIDGE_IDENTITY_FILE",
        "BRIDGE_MCP_TOKEN",
        "BRIDGE_MCP_SOCKET",
        "BUILD_PI_MCP_OWNER",
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
    ] {
        device.set(name, "withheld");
    }
    device.inventory().sweep();

    let calls = std::fs::read_to_string(device.home().join("claude-calls")).unwrap();
    assert_eq!(calls, "auth status\n");
    let cwd = std::fs::read_to_string(device.home().join("claude-cwd")).unwrap();
    assert_eq!(
        std::fs::canonicalize(cwd.trim()).unwrap(),
        std::fs::canonicalize(device.home()).unwrap()
    );
    let environment = std::fs::read_to_string(device.home().join("claude-env")).unwrap();
    assert!(!environment.contains("withheld"), "{environment}");
    for expected in [
        "MISE_OFFLINE=1",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1",
    ] {
        assert!(
            environment.lines().any(|line| line == expected),
            "{environment}"
        );
    }
}

#[test]
fn an_expired_claude_access_token_with_a_refresh_token_is_refresh_pending() {
    let device = Device::new();
    device.install_claude();
    device.write("claude-status.json", SIGNED_IN_WITH_CLAUDE_AI);
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
    assert_eq!(
        context(&snapshot, "claude").facts.evidence,
        [Evidence::CliStatus, Evidence::CredentialsFile]
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
fn a_configured_claude_helper_is_never_run_and_neither_is_the_status_command() {
    let device = Device::new();
    device.install_claude();
    device.write("claude-status.json", SIGNED_IN_WITH_CLAUDE_AI);
    let helper = device.home().join("helper");
    write_executable(
        &helper,
        "#!/bin/sh\ntouch \"$HOME/helper-ran\"\necho sk-ant-key\n",
    );
    device.write(
        ".claude/settings.json",
        &format!(r#"{{"apiKeyHelper": "{}"}}"#, helper.display()),
    );
    let inventory = device.inventory();
    inventory.sweep();

    let snapshot = inventory.snapshot();
    let claude = context(&snapshot, "claude");
    assert_eq!(
        (claude.facts.method, claude.facts.status),
        (AuthMethod::External, AuthStatus::Unknown)
    );
    assert_eq!(claude.facts.evidence, [Evidence::Settings]);
    assert_eq!(device.claude_calls(), 0);
    assert!(!device.home().join("helper-ran").exists());

    // Policy settings count the same as the user's own.
    std::fs::remove_file(device.home().join(".claude/settings.json")).unwrap();
    device.write(
        "managed-settings.json",
        r#"{"awsAuthRefresh": "aws sso login"}"#,
    );
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    assert_eq!(device.claude_calls(), 0);
}

#[test]
fn claude_without_its_cli_reads_the_environment_by_name_only() {
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
    let snapshot = inventory.snapshot();
    assert_eq!(
        (
            context(&snapshot, "codex").facts.method,
            context(&snapshot, "codex").facts.status
        ),
        (AuthMethod::None, AuthStatus::NotSignedIn)
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

    assert!(!device.home().join("codex-ran").exists());
    assert!(!serde_json::to_string(&snapshot).unwrap().contains("SECRET"));
}

#[test]
fn codex_credentials_in_the_keyring_are_unknown_rather_than_absent() {
    let device = Device::new();
    device.write(
        ".codex/config.toml",
        "model = \"gpt-6\"\ncli_auth_credentials_store = \"keyring\"\n",
    );
    let inventory = device.inventory();
    inventory.sweep();
    let snapshot = inventory.snapshot();
    assert_eq!(
        context(&snapshot, "codex").facts.status,
        AuthStatus::Unknown
    );
}

#[test]
fn pi_lists_each_provider_without_resolving_keys_or_running_commands() {
    let device = Device::new();
    device.install_tripwire("pi", "pi-ran");
    let command = device.home().join("key-command");
    write_executable(&command, "#!/bin/sh\ntouch \"$HOME/key-command-ran\"\n");
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
    assert!(!device.home().join("key-command-ran").exists());
    assert!(!device.home().join("pi-ran").exists());
    let wire = serde_json::to_string(&snapshot).unwrap();
    assert!(
        !wire.contains("SECRET") && !wire.contains("script"),
        "{wire}"
    );
}

#[test]
fn installs_removals_and_retargeted_links_are_seen_and_check_sign_in_again() {
    let device = Device::new();
    device.write("claude-status.json", SIGNED_IN_WITH_CLAUDE_AI);
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

    let first = device.home().join("versions/claude-a");
    device.install_claude_at(&first);
    std::os::unix::fs::symlink(&first, device.home().join("bin/claude")).unwrap();
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    let snapshot = inventory.snapshot();
    let adk = harness(&snapshot, AgentProvider::ClaudeAdk);
    assert_eq!(adk.installation.state, InstallationState::Installed);
    assert_eq!(adk.version.installed.as_deref(), Some("2.1.284"));
    assert_eq!(device.claude_calls(), 1, "the install was checked at once");

    let second = device.home().join("versions/claude-b");
    device.install_claude_at(&second);
    std::fs::remove_file(device.home().join("bin/claude")).unwrap();
    std::os::unix::fs::symlink(&second, device.home().join("bin/claude")).unwrap();
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    assert_eq!(
        device.claude_calls(),
        2,
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
}

#[test]
fn installed_and_running_versions_are_reported_apart_and_unknown_stays_unknown() {
    let device = Device::new();
    device.install_claude();
    device.write("claude-status.json", SIGNED_IN_WITH_CLAUDE_AI);
    device.install_tripwire("codex", "codex-ran");
    let running = RunningSessions::new();
    let inventory = device.builder().running(Arc::clone(&running)).build();
    let mut changes = inventory.changes();
    inventory.sweep();
    changes.mark_unchanged();
    let session = running.report(AgentProvider::ClaudeAdk, "2.1.280".into());
    let _other = running.report(AgentProvider::ClaudeAdk, "2.1.280".into());
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
    drop(_other);
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
    device.install_claude();
    device.write("claude-status.json", SIGNED_IN_WITH_CLAUDE_AI);
    let inventory = device.inventory();
    inventory.sweep();
    device.advance(SWEEP_INTERVAL);
    inventory.sweep();
    assert_eq!(device.claude_calls(), 1);
    device.advance(AUTH_CHECK_INTERVAL);
    inventory.sweep();
    assert_eq!(device.claude_calls(), 2);

    // The CLI starts answering nonsense: what it said before stands, stale.
    device.write("claude-status.json", "Something went wrong");
    device.advance(AUTH_CHECK_INTERVAL);
    inventory.sweep();
    assert_eq!(device.claude_calls(), 3);
    let snapshot = inventory.snapshot();
    let claude = context(&snapshot, "claude");
    assert_eq!(claude.facts.status, AuthStatus::SignedIn);
    assert_eq!(claude.health, Health::Stale);
    // The failed check waits a minute, the next one two.
    device.advance(AUTH_CHECK_INTERVAL);
    inventory.sweep();
    assert_eq!(device.claude_calls(), 4);
    device.advance(AUTH_CHECK_INTERVAL);
    inventory.sweep();
    assert_eq!(device.claude_calls(), 4);
    device.advance(AUTH_CHECK_INTERVAL);
    inventory.sweep();
    assert_eq!(device.claude_calls(), 5);
    assert_eq!(backoff(20), MAX_FAILURE_BACKOFF);
}

#[test]
fn a_refresh_answers_at_once_and_completes_with_the_next_sweep() {
    let device = Device::new();
    device.install_claude();
    device.write("claude-status.json", SIGNED_IN_WITH_CLAUDE_AI);
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
    assert_eq!(device.claude_calls(), 1, "a refresh never waits on a CLI");

    // Inside the floor the check waits, and the refresh stays open.
    inventory.sweep();
    assert_eq!(inventory.snapshot().refresh.completed, 0);
    device.advance(FORCED_CHECK_FLOOR);
    inventory.sweep();
    let after = inventory.snapshot();
    assert_eq!(device.claude_calls(), 2);
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

#[test]
fn the_sweep_runs_with_no_client_connected() {
    let device = Device::new();
    device.install_claude();
    device.write("claude-status.json", SIGNED_IN_WITH_CLAUDE_AI);
    let inventory = device
        .builder()
        .clocks(Instant::now, SystemTime::now)
        .sweep_every(Duration::from_millis(20))
        .build();
    inventory.start();
    let deadline = Instant::now() + Duration::from_secs(20);
    while context(&inventory.snapshot(), "claude").health != Health::Fresh {
        assert!(Instant::now() < deadline, "no sweep ran");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(device.claude_calls(), 1);
}

#[test]
fn a_restart_restores_nonsecret_observations_as_stale() {
    let device = Device::new();
    device.install_claude();
    device.write("claude-status.json", SIGNED_IN_WITH_CLAUDE_AI);
    device.write(
        ".claude/.credentials.json",
        r#"{"claudeAiOauth": {"accessToken": "sk-ant-oat-SECRET", "refreshToken": "rt-SECRET", "expiresAt": 99999999999999}}"#,
    );
    let saved = device.home().join("state/harness-inventory.json");
    std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
    let inventory = device.builder().persist_to(saved.clone()).build();
    inventory.sweep();
    let before = inventory.snapshot();

    let written = std::fs::read_to_string(&saved).unwrap();
    for private in ["SECRET", "person@example.com", "Example Org"] {
        assert!(!written.contains(private), "{written}");
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
