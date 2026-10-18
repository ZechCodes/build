//! Observe local managed policy without executing policy helpers or exposing values.
use std::path::Path;

pub(super) fn launch_conflict() -> Option<String> {
    None
}

fn inspect(_system: &Path, _config: &Path) -> Option<String> {
    None
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
            fs::write(root.path().join("system/managed-settings.json"), document.to_string()).unwrap();
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
        for key in ["autoCompactEnabled", "autoCompactWindow", "precomputeCompactionEnabled"] {
            for value in [json!(false), json!(null)] {
                fs::write(root.path().join("system/managed-settings.json"), json!({key: value}).to_string()).unwrap();
                assert!(conflict(root.path()).unwrap().contains(key));
            }
        }
    }

    #[test]
    fn managed_policy_checks_fragments_and_remote_cache_but_ignores_unrelated_settings() {
        let root = fixture();
        assert_eq!(conflict(root.path()), None);
        fs::write(root.path().join("system/managed-settings.json"), r#"{"theme":"dark","env":{"UNRELATED":"value"}}"#).unwrap();
        assert_eq!(conflict(root.path()), None);
        let fragments = root.path().join("system/managed-settings.d");
        fs::create_dir(&fragments).unwrap();
        fs::write(fragments.join(".hidden.json"), "invalid").unwrap();
        fs::write(fragments.join("ignored.txt"), "invalid").unwrap();
        assert_eq!(conflict(root.path()), None);
        fs::write(fragments.join("10-policy.json"), r#"{"autoCompactWindow":150000}"#).unwrap();
        assert!(conflict(root.path()).unwrap().contains("autoCompactWindow"));
        fs::remove_file(fragments.join("10-policy.json")).unwrap();
        fs::write(root.path().join("config/remote-settings.json"), r#"{"env":{"DISABLE_COMPACT":"1"}}"#).unwrap();
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
    fn managed_policy_helpers_and_model_budget_overrides_are_unverified() {
        let root = fixture();
        for key in ["policyHelper", "policyHelpers", "modelOverrides", "fallbackModel", "model"] {
            fs::write(root.path().join("system/managed-settings.json"), json!({key: {"secret": "not-executed"}}).to_string()).unwrap();
            let reason = conflict(root.path()).unwrap();
            assert!(reason.contains(key) && reason.contains("best effort"));
            assert!(!reason.contains("not-executed"));
        }
    }

    #[test]
    fn managed_policy_reads_and_fragment_count_are_bounded() {
        let root = fixture();
        let path = root.path().join("system/managed-settings.json");
        fs::File::create(&path).unwrap().set_len(8 * 1024 * 1024 + 1).unwrap();
        assert!(conflict(root.path()).unwrap().contains("unreadable"));
        fs::remove_file(path).unwrap();
        let fragments = root.path().join("system/managed-settings.d");
        fs::create_dir(&fragments).unwrap();
        for index in 0..257 {
            fs::write(fragments.join(format!("{index}.json")), "{}").unwrap();
        }
        assert!(conflict(root.path()).unwrap().contains("unreadable"));
    }
}
