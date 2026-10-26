use crate::compaction::{CompactionSource, ResolvedCompactionPolicy, MAX_SAFE_TOKENS};
use crate::harness::compaction::{CompactionEnforcement, CompactionLaunchIdentity};
use crate::harness::pi::PiHarness;
use crate::harness::Harness;
use crate::models::{AgentProvider, ModelChoice};

fn choice() -> ModelChoice {
    ModelChoice {
        provider: AgentProvider::Pi,
        model: Some("test-provider/test-model".into()),
        effort: Some("high".into()),
    }
}

#[test]
fn pi_compaction_qualifies_hooks_but_delegates_native_threshold() {
    assert!(PiHarness.compacts_on_command());
    for tokens in [1, 150_000, 200_000, MAX_SAFE_TOKENS] {
        let identity = CompactionLaunchIdentity::new("thread-one");
        let policy = PiHarness
            .prepare_compaction_policy(
                ResolvedCompactionPolicy {
                    tokens,
                    source: CompactionSource::Project("project-one".into()),
                },
                choice(),
                Some("0.86.1".into()),
                identity.clone(),
            )
            .unwrap();
        assert_eq!(policy.enforcement, CompactionEnforcement::Native);
        assert_eq!(policy.requested.tokens, tokens);
        assert_eq!(policy.launch_identity, identity);
        assert_eq!(policy.choice, choice());
        assert!(!policy.capabilities.native_threshold);
        assert!(policy.capabilities.between_turn_compact);
        assert_eq!(
            policy.reason.as_deref(),
            Some("pi_native_threshold_injection_unqualified")
        );
        assert_eq!(policy.translated_trigger, None);
        assert_eq!(policy.native_settings, None);
        assert!(!policy.automatic_due(None));
        assert!(!policy.automatic_due(Some(tokens - 1)));
        assert!(!policy.automatic_due(Some(tokens)));
    }
}

#[test]
fn pi_compaction_unknown_version_is_unsupported_without_rounding_intent() {
    for version in [None, Some("0.86.0"), Some("0.86.2"), Some("0.86.1-beta")] {
        let policy = PiHarness
            .prepare_compaction_policy(
                ResolvedCompactionPolicy {
                    tokens: 150_000,
                    source: CompactionSource::Device,
                },
                choice(),
                version.map(str::to_owned),
                CompactionLaunchIdentity::new("thread"),
            )
            .unwrap();
        assert_eq!(policy.enforcement, CompactionEnforcement::Unsupported);
        assert!(!policy.capabilities.between_turn_compact);
        assert!(!policy.capabilities.native_threshold);
        assert_eq!(policy.reason.as_deref(), Some("pi_version_unqualified"));
        assert_eq!(policy.requested.tokens, 150_000);
        assert!(!policy.automatic_due(Some(200_000)));
    }
}

#[test]
fn pi_compaction_preserves_unsafe_legacy_values_and_rejects_zero() {
    for tokens in [MAX_SAFE_TOKENS + 1, u64::MAX] {
        let policy = PiHarness
            .prepare_compaction_policy(
                ResolvedCompactionPolicy {
                    tokens,
                    source: CompactionSource::Device,
                },
                choice(),
                Some("0.86.1".into()),
                CompactionLaunchIdentity::new("thread"),
            )
            .unwrap();
        assert_eq!(policy.enforcement, CompactionEnforcement::Unsupported);
        assert_eq!(policy.reason.as_deref(), Some("unsupported_representation"));
        assert_eq!(policy.requested.tokens, tokens);
    }
    assert!(PiHarness
        .prepare_compaction_policy(
            ResolvedCompactionPolicy {
                tokens: 0,
                source: CompactionSource::Device
            },
            choice(),
            Some("0.86.1".into()),
            CompactionLaunchIdentity::new("thread"),
        )
        .is_err());
}

fn options(
    cwd: &std::path::Path,
    owner: &str,
    version: Option<&str>,
    tokens: u64,
) -> crate::orchestrator::SpawnOptions {
    crate::orchestrator::SpawnOptions {
        cwd: cwd.to_owned(),
        owner_id: owner.into(),
        compaction_policy: Some(
            PiHarness
                .prepare_compaction_policy(
                    ResolvedCompactionPolicy {
                        tokens,
                        source: CompactionSource::Device,
                    },
                    choice(),
                    version.map(str::to_owned),
                    CompactionLaunchIdentity::new("thread:agent-one:reset-generation"),
                )
                .unwrap(),
        ),
        ..Default::default()
    }
}

fn context(state: &std::path::Path) -> crate::harness::HarnessContext {
    crate::harness::HarnessContext {
        channel: crate::config::Channel::Prod,
        state_root: state.to_owned(),
        bridge_exe: "/fake/bridge".into(),
        mcp_socket: state.join("socket"),
    }
}

fn env(spec: &crate::pty::HarnessSpec, key: &str) -> String {
    spec.env.iter().find(|(k, _)| k == key).unwrap().1.clone()
}

fn assert_scoped_launch(
    spec: &crate::pty::HarnessSpec,
    options: &crate::orchestrator::SpawnOptions,
    state: &std::path::Path,
) -> std::path::PathBuf {
    let owner = options.owner_id.as_str();
    let policy: serde_json::Value =
        serde_json::from_str(&env(spec, "BUILD_PI_COMPACTION_POLICY")).unwrap();
    assert_eq!(policy["tokens"], 150_000);
    assert_eq!(policy["owner"], owner);
    assert_eq!(policy["thread_id"], "thread:agent-one:reset-generation");
    assert_eq!(
        policy["launch_id"],
        options
            .compaction_policy
            .as_ref()
            .unwrap()
            .launch_identity
            .id
    );
    assert_eq!(policy["enabled"], false);
    assert_eq!(spec.known_session_id.as_deref(), Some(owner));
    assert!(spec
        .args
        .windows(2)
        .any(|pair| pair == ["--session-id", owner]));
    assert!(!spec
        .args
        .iter()
        .any(|arg| arg == "--continue" || arg == "--resume"));
    let telemetry = spec.pi_telemetry.as_ref().unwrap();
    assert_eq!(telemetry.policy, options.compaction_policy.clone().unwrap());
    assert_eq!(telemetry.owner, owner);
    assert_eq!(
        telemetry.path.to_string_lossy(),
        env(spec, "BUILD_PI_TELEMETRY_LOG")
    );
    assert!(telemetry
        .path
        .starts_with(state.join("harness/pi/sessions").join(owner)));
    telemetry.path.clone()
}

#[test]
fn pi_compaction_spec_preserves_exact_session_and_scopes_each_launch() {
    let state = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let context = context(state.path());
    let mut paths = std::collections::HashSet::new();
    for owner in ["agent-one", "agent-two"] {
        for resume in [None, Some("continue"), Some("exact-session")] {
            let mut options = options(checkout.path(), owner, Some("0.86.1"), 150_000);
            options.continue_session = resume == Some("continue");
            options.resume_session_id = resume.filter(|r| *r != "continue").map(str::to_owned);
            let spec = PiHarness.spec(&choice(), &options, &context).unwrap();
            assert!(paths.insert(assert_scoped_launch(&spec, &options, state.path())));
        }
    }
    assert_eq!(std::fs::read_dir(checkout.path()).unwrap().count(), 0);
}

#[test]
fn pi_compaction_spec_keeps_unsafe_value_telemetry_without_enabling_cancellation() {
    let state = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let options = options(checkout.path(), "agent", Some("0.86.1"), u64::MAX);
    let spec = PiHarness
        .spec(&choice(), &options, &context(state.path()))
        .unwrap();
    let policy: serde_json::Value =
        serde_json::from_str(&env(&spec, "BUILD_PI_COMPACTION_POLICY")).unwrap();
    assert_eq!(policy["enabled"], false);
    assert!(spec.pi_telemetry.is_some());
    assert!(policy["tokens"].is_null());
    assert_eq!(policy["tokens_decimal"], u64::MAX.to_string());
}

#[test]
fn pi_compaction_unknown_hook_version_keeps_legacy_terminal_lifecycle() {
    let state = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    for version in [None, Some("0.86.0"), Some("0.86.2")] {
        let options = options(checkout.path(), "agent", version, 150_000);
        let spec = PiHarness
            .spec(&choice(), &options, &context(state.path()))
            .unwrap();
        assert!(spec.pi_telemetry.is_none());
        for key in ["BUILD_PI_COMPACTION_POLICY", "BUILD_PI_TELEMETRY_LOG"] {
            assert!(spec.unset.iter().any(|item| item == key));
            assert!(!spec.env.iter().any(|(item, _)| item == key));
        }
        assert_eq!(
            options.compaction_policy.unwrap().reason.as_deref(),
            Some("pi_version_unqualified")
        );
    }
}

#[test]
fn pi_compaction_spec_rejects_forged_policy_and_identity_before_writing() {
    for mutation in 0..9 {
        let state = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let mut options = options(checkout.path(), "agent-one", Some("0.86.1"), 150_000);
        let policy = options.compaction_policy.as_mut().unwrap();
        match mutation {
            0 => policy.choice.model = Some("another-model".into()),
            1 => policy.enforcement = CompactionEnforcement::BetweenTurn,
            2 => policy.translated_trigger = Some(150_000),
            3 => policy.launch_identity.id = "../launch".into(),
            4 => policy.launch_identity.thread_id = "".into(),
            5 => policy.capabilities.qualification.version = None,
            6 => options.owner_id = "../owner".into(),
            7 => policy.launch_identity.thread_id = "thread:agent\nforged".into(),
            8 => policy.launch_identity.thread_id = "x".repeat(1025),
            _ => unreachable!(),
        }
        assert!(
            PiHarness
                .spec(&choice(), &options, &context(state.path()))
                .is_err(),
            "mutation {mutation}"
        );
        assert_eq!(
            std::fs::read_dir(state.path()).unwrap().count(),
            0,
            "mutation {mutation}"
        );
    }
}

#[test]
fn pi_compaction_legacy_spec_clears_parent_policy_without_inventing_snapshot() {
    let state = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let mut options = options(checkout.path(), "legacy-agent", Some("0.86.1"), 150_000);
    options.compaction_policy = None;
    let spec = PiHarness
        .spec(&choice(), &options, &context(state.path()))
        .unwrap();
    assert!(spec.pi_telemetry.is_none());
    for key in ["BUILD_PI_COMPACTION_POLICY", "BUILD_PI_TELEMETRY_LOG"] {
        assert!(spec.unset.iter().any(|item| item == key));
        assert!(!spec.env.iter().any(|(item, _)| item == key));
    }
}
