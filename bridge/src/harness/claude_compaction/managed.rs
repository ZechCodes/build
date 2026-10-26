//! Observe local managed policy without executing policy helpers or exposing values.
//!
//! This is a launch-time observation of local sources, not a readback of the
//! child's effective policy: a later server refresh can replace cached policy.
use serde_json::Value;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 256;
const UNREADABLE: &str = "best effort, managed policy unreadable or inspection bound exceeded";
const SETTING_KEYS: &[&str] = &[
    "autoCompactEnabled",
    "autoCompactWindow",
    "precomputeCompactionEnabled",
    "policyHelper",
    "policyHelpers",
    "model",
    "modelOverrides",
    "availableModels",
    "deniedModels",
    "fallbackModel",
];
const ENV_KEYS: &[&str] = &[
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
    "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
    "DISABLE_AUTO_COMPACT",
    "DISABLE_COMPACT",
    "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
    "CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE",
    "CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT",
];

#[derive(Clone, Debug, Default)]
pub(super) struct Observation {
    pub(super) reason: Option<String>,
    pub(super) disables_auto: bool,
    pub(super) disables_command: bool,
}

#[derive(Default)]
struct DisableFlags {
    auto_setting: bool,
    auto_env: bool,
    command: bool,
}

#[cfg(test)]
thread_local! {
    static LAUNCH_ROOTS: std::cell::RefCell<Option<(PathBuf, PathBuf)>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Test roots stay on the preparing thread and restore the enclosing scope.
#[cfg(test)]
pub(super) struct ScopedLaunchRoots {
    previous: Option<(PathBuf, PathBuf)>,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(test)]
impl Drop for ScopedLaunchRoots {
    fn drop(&mut self) {
        LAUNCH_ROOTS.with(|roots| *roots.borrow_mut() = self.previous.take());
    }
}

#[cfg(test)]
pub(super) fn scoped_launch_roots(system: &Path, config: &Path) -> ScopedLaunchRoots {
    let previous = LAUNCH_ROOTS.with(|roots| {
        roots
            .borrow_mut()
            .replace((system.to_owned(), config.to_owned()))
    });
    ScopedLaunchRoots {
        previous,
        _thread_bound: std::marker::PhantomData,
    }
}

#[cfg(test)]
pub(super) fn launch_conflict() -> Observation {
    // Unscoped unit/child tests never inspect the developer's configuration.
    // Scoped launch tests use the same source reader as production.
    let roots = LAUNCH_ROOTS.with(|roots| roots.borrow().clone());
    roots.map_or_else(Observation::default, |(system, config)| {
        observe(&system, &config)
    })
}

#[cfg(not(test))]
pub(super) fn launch_conflict() -> Observation {
    if !cfg!(target_os = "linux")
        || std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::env::var_os("WSL_INTEROP").is_some()
    {
        return Observation {
            reason: Some("best effort, managed platform policy inspection unverified".into()),
            ..Observation::default()
        };
    }
    let config = config_path(
        std::env::var_os("CLAUDE_CONFIG_DIR"),
        std::env::var_os("HOME"),
    );
    match config {
        Some(config) => observe(Path::new("/etc/claude-code"), &config),
        None => Observation { reason: Some("best effort, managed policy inspection unverified: config directory unavailable or nonabsolute".into()), ..Observation::default() },
    }
}

fn config_path(config: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    config
        .map(PathBuf::from)
        .or_else(|| home.map(|home| PathBuf::from(home).join(".claude")))
        .filter(|path| path.is_absolute())
}

#[cfg(test)]
fn inspect(system: &Path, config: &Path) -> Option<String> {
    observe(system, config).reason
}

pub(super) fn observe(system: &Path, config: &Path) -> Observation {
    let (keys, unreadable, disabled) = inspect_sources(system, config);
    Observation {
        reason: inspection_reason(keys, unreadable),
        disables_auto: disabled.auto_setting || disabled.auto_env || disabled.command,
        disables_command: disabled.command,
    }
}

fn inspection_reason(keys: BTreeSet<&'static str>, unreadable: bool) -> Option<String> {
    if keys.is_empty() {
        return unreadable.then(|| UNREADABLE.into());
    }
    let mut reason = format!(
        "best effort, managed policy present: {}",
        keys.into_iter().collect::<Vec<_>>().join(", ")
    );
    if unreadable {
        reason.push_str("; additional managed policy unreadable or inspection bound exceeded");
    }
    Some(reason)
}

fn inspect_sources(system: &Path, config: &Path) -> (BTreeSet<&'static str>, bool, DisableFlags) {
    let mut keys = BTreeSet::new();
    let mut disabled = DisableFlags::default();
    let mut budget = MAX_TOTAL_BYTES;
    let mut unreadable = inspect_file(
        &system.join("managed-settings.json"),
        &mut budget,
        &mut keys,
        &mut disabled,
    )
    .is_err();
    match fragments(&system.join("managed-settings.d")) {
        Ok(paths) => {
            for path in paths {
                unreadable |= inspect_file(&path, &mut budget, &mut keys, &mut disabled).is_err();
            }
        }
        Err(()) => unreadable = true,
    }
    // A raw remote cache does not prove consent/eligibility or the live tier.
    // Keep its key-name warning, but never schedule a competing fallback from it.
    unreadable |= inspect_file(
        &config.join("remote-settings.json"),
        &mut budget,
        &mut keys,
        &mut DisableFlags::default(),
    )
    .is_err();
    (keys, unreadable, disabled)
}

fn fragments(directory: &Path) -> Result<Vec<PathBuf>, ()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(()),
    };
    let mut paths = Vec::new();
    for (index, entry) in entries.take(MAX_DIRECTORY_ENTRIES + 1).enumerate() {
        if index == MAX_DIRECTORY_ENTRIES {
            return Err(());
        }
        let entry = entry.map_err(|_| ())?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(())?;
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        let kind = entry.file_type().map_err(|_| ())?;
        if kind.is_file() || kind.is_symlink() {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn inspect_file(
    path: &Path,
    budget: &mut u64,
    keys: &mut BTreeSet<&'static str>,
    disabled: &mut DisableFlags,
) -> Result<(), ()> {
    let Some(bytes) = read_bounded(path, budget)? else {
        return Ok(());
    };
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    let document: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    let document = document.as_object().ok_or(())?;
    keys.extend(
        SETTING_KEYS
            .iter()
            .copied()
            .filter(|key| document.contains_key(*key)),
    );
    if let Some(enabled) = document.get("autoCompactEnabled").and_then(Value::as_bool) {
        disabled.auto_setting = !enabled;
    }
    if let Some(env) = document.get("env") {
        let env = env.as_object().ok_or(())?;
        if let Some(value) = env.get("DISABLE_AUTO_COMPACT") {
            disabled.auto_env = env_truthy(Some(value));
        }
        if let Some(value) = env.get("DISABLE_COMPACT") {
            disabled.command = env_truthy(Some(value));
        }
        keys.extend(
            ENV_KEYS
                .iter()
                .copied()
                .filter(|key| env.contains_key(*key)),
        );
    }
    Ok(())
}

fn env_truthy(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn read_bounded(path: &Path, budget: &mut u64) -> Result<Option<Vec<u8>>, ()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    }
    let mut options = OpenOptions::new();
    options.read(true);
    // A policy path may be replaced between inspection and open. Nonblocking
    // open followed by descriptor metadata refuses FIFOs/devices without a hang.
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let file = options.open(path).map_err(|_| ())?;
    let metadata = file.metadata().map_err(|_| ())?;
    let limit = MAX_FILE_BYTES.min(*budget);
    if !metadata.is_file() || metadata.len() > limit {
        return Err(());
    }
    read_with_budget(file, budget).map(Some)
}

fn read_with_budget(reader: impl Read, budget: &mut u64) -> Result<Vec<u8>, ()> {
    // The extra byte detects growth beyond the per-file cap, but must itself
    // fit the aggregate cap. Reaching that cap without proving EOF fails closed.
    let limit = (MAX_FILE_BYTES + 1).min(*budget);
    if limit == 0 {
        return Err(());
    }
    let mut bytes = Vec::new();
    let result = reader.take(limit).read_to_end(&mut bytes);
    *budget -= bytes.len() as u64;
    result.map_err(|_| ())?;
    if bytes.len() as u64 == limit {
        return Err(());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("system")).unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        root
    }

    fn conflict(root: &Path) -> Option<String> {
        inspect(&root.join("system"), &root.join("config"))
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("injected read failure"))
        }
    }

    #[test]
    fn managed_policy_partial_read_failure_debits_consumed_bytes() {
        let mut budget = 10;
        let reader = std::io::Cursor::new(b"partial").chain(FailingReader);
        assert!(read_with_budget(reader, &mut budget).is_err());
        assert_eq!(budget, 3);
        assert_eq!(
            read_with_budget(&b"{}"[..], &mut budget),
            Ok(b"{}".to_vec())
        );
        assert_eq!(budget, 1);
    }

    #[test]
    fn managed_policy_growth_past_file_limit_debits_the_oversize_probe() {
        let mut budget = MAX_TOTAL_BYTES;
        // A descriptor may grow after its metadata passed the initial limit.
        let mut reader = std::io::repeat(b' ');
        assert!(read_with_budget(&mut reader, &mut budget).is_err());
        assert_eq!(budget, MAX_TOTAL_BYTES - MAX_FILE_BYTES - 1);
    }

    #[test]
    fn managed_policy_exhausted_budget_does_not_read_more_probe_bytes() {
        let mut budget = 3;
        let mut reader = std::io::Cursor::new(b"longer than the budget");
        assert!(read_with_budget(&mut reader, &mut budget).is_err());
        assert_eq!(reader.position(), 3);
        assert_eq!(budget, 0);
        assert!(read_with_budget(&mut reader, &mut budget).is_err());
        assert_eq!(reader.position(), 3);
    }

    #[test]
    fn managed_policy_config_paths_never_assume_the_child_working_directory() {
        let absolute = std::env::temp_dir();
        let home = Some(absolute.clone().into_os_string());
        assert_eq!(
            config_path(None, home.clone()),
            Some(absolute.join(".claude"))
        );
        for config in ["", ".", "relative"] {
            assert_eq!(config_path(Some(config.into()), home.clone()), None);
        }
        assert_eq!(config_path(None, Some("relative".into())), None);
        assert_eq!(config_path(None, None), None);
        assert_eq!(config_path(home, Some("relative".into())), Some(absolute));
    }

    #[test]
    fn managed_policy_names_conflicting_keys_without_values_or_paths() {
        let root = fixture();
        for key in [
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
            "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
            "DISABLE_AUTO_COMPACT",
            "DISABLE_COMPACT",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        ] {
            let document = json!({"env": {key: "private-policy-value"}});
            fs::write(
                root.path().join("system/managed-settings.json"),
                document.to_string(),
            )
            .unwrap();
            let reason = conflict(root.path()).unwrap();
            assert!(reason.contains(key), "{reason}");
            assert!(reason.contains("best effort, managed policy present"));
            assert!(!reason.contains("private-policy-value"));
            assert!(!reason.contains(root.path().to_str().unwrap()));
        }
    }

    #[test]
    fn managed_policy_detects_settings_even_when_false_or_null() {
        let root = fixture();
        for key in [
            "autoCompactEnabled",
            "autoCompactWindow",
            "precomputeCompactionEnabled",
        ] {
            for value in [json!(false), json!(null)] {
                fs::write(
                    root.path().join("system/managed-settings.json"),
                    json!({key: value}).to_string(),
                )
                .unwrap();
                assert!(conflict(root.path()).unwrap().contains(key));
            }
        }
    }

    #[test]
    fn managed_policy_checks_fragments_and_remote_cache_but_ignores_unrelated_settings() {
        let root = fixture();
        assert_eq!(conflict(root.path()), None);
        fs::write(
            root.path().join("system/managed-settings.json"),
            r#"{"theme":"dark","env":{"UNRELATED":"value"}}"#,
        )
        .unwrap();
        assert_eq!(conflict(root.path()), None);
        let fragments = root.path().join("system/managed-settings.d");
        fs::create_dir(&fragments).unwrap();
        fs::write(fragments.join(".hidden.json"), "invalid").unwrap();
        fs::write(fragments.join("ignored.txt"), "invalid").unwrap();
        assert_eq!(conflict(root.path()), None);
        fs::write(
            fragments.join("10-policy.json"),
            r#"{"autoCompactWindow":150000}"#,
        )
        .unwrap();
        assert!(conflict(root.path()).unwrap().contains("autoCompactWindow"));
        fs::remove_file(fragments.join("10-policy.json")).unwrap();
        fs::write(
            root.path().join("config/remote-settings.json"),
            r#"{"env":{"DISABLE_COMPACT":"1"}}"#,
        )
        .unwrap();
        assert!(conflict(root.path()).unwrap().contains("DISABLE_COMPACT"));
    }

    #[test]
    fn managed_policy_unknown_documents_are_best_effort_unreadable() {
        let root = fixture();
        for document in ["{", "[]", "null", r#"{"env":false}"#] {
            fs::write(root.path().join("system/managed-settings.json"), document).unwrap();
            let reason = conflict(root.path()).unwrap();
            assert!(reason.contains("best effort") && reason.contains("unreadable"));
        }
        fs::remove_file(root.path().join("system/managed-settings.json")).unwrap();
        fs::create_dir(root.path().join("system/managed-settings.json")).unwrap();
        assert!(conflict(root.path()).unwrap().contains("unreadable"));
    }

    #[test]
    fn managed_policy_preserves_named_keys_alongside_unreadable_sources() {
        let root = fixture();
        let named = r#"{"env":{"DISABLE_AUTO_COMPACT":"0"}}"#;
        for (system, remote) in [(named, "{"), ("{", named)] {
            fs::write(root.path().join("system/managed-settings.json"), system).unwrap();
            fs::write(root.path().join("config/remote-settings.json"), remote).unwrap();
            let reason = conflict(root.path()).unwrap();
            assert!(reason.contains("DISABLE_AUTO_COMPACT"), "{reason}");
            assert!(reason.contains("best effort") && reason.contains("unreadable"));
        }
    }

    #[test]
    fn managed_policy_helpers_and_model_budget_overrides_are_unverified() {
        let root = fixture();
        for key in [
            "policyHelper",
            "policyHelpers",
            "modelOverrides",
            "fallbackModel",
            "model",
        ] {
            fs::write(
                root.path().join("system/managed-settings.json"),
                json!({key: {"secret": "not-executed"}}).to_string(),
            )
            .unwrap();
            let reason = conflict(root.path()).unwrap();
            assert!(reason.contains(key) && reason.contains("best effort"));
            assert!(!reason.contains("not-executed"));
        }
    }

    #[test]
    fn managed_policy_reads_and_fragment_count_are_bounded() {
        let root = fixture();
        let path = root.path().join("system/managed-settings.json");
        fs::File::create(&path)
            .unwrap()
            .set_len(8 * 1024 * 1024 + 1)
            .unwrap();
        assert!(conflict(root.path()).unwrap().contains("unreadable"));
        fs::remove_file(path).unwrap();
        let fragments = root.path().join("system/managed-settings.d");
        fs::create_dir(&fragments).unwrap();
        for index in 0..257 {
            fs::write(fragments.join(format!("{index}.json")), "{}").unwrap();
        }
        assert!(conflict(root.path()).unwrap().contains("unreadable"));
    }

    #[cfg(unix)]
    #[test]
    fn managed_policy_regular_symlinks_are_checked_and_special_files_refused() {
        use std::os::unix::{fs::symlink, net::UnixListener};

        let root = fixture();
        let target = root.path().join("policy.json");
        let policy = root.path().join("system/managed-settings.json");
        fs::write(&target, r#"{"env":{"DISABLE_AUTO_COMPACT":"0"}}"#).unwrap();
        symlink(&target, &policy).unwrap();
        assert!(conflict(root.path())
            .unwrap()
            .contains("DISABLE_AUTO_COMPACT"));
        fs::remove_file(&target).unwrap();
        assert!(conflict(root.path()).unwrap().contains("unreadable"));
        fs::remove_file(&policy).unwrap();
        let _socket = UnixListener::bind(&policy).unwrap();
        assert!(conflict(root.path()).unwrap().contains("unreadable"));
    }
}
