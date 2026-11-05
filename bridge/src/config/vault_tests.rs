use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "synthetic-vault-config-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn configured(home: &Path, channel: &str, key: &str, value: &str) -> Result<BridgeConfig, String> {
    resolve(
        |name| match name {
            "BRIDGE_CHANNEL" => Some(channel.to_string()),
            _ if name == key => Some(value.to_string()),
            _ => None,
        },
        home,
    )
}

#[test]
fn vault_paths_refuse_tree_roots_containing_or_inside_vault_in_both_channels() {
    let fixture = Fixture::new();
    for (channel, root) in [("prod", ".build"), ("dev", ".local/state/build-dev")] {
        let state = fixture.0.join(root);
        for key in [
            "BRIDGE_TASKS_DIR",
            "BRIDGE_PROJECTS_DIR",
            "BRIDGE_WORKTREES",
            "BRIDGE_REPO",
            "BRIDGE_PROJECTS",
        ] {
            for path in [
                &fixture.0,
                &state,
                &state.join("secrets"),
                &state.join("secrets/nested"),
            ] {
                let error =
                    configured(&fixture.0, channel, key, path.to_str().unwrap()).unwrap_err();
                assert!(error.contains("vault"), "{key}: {error}");
            }
        }
    }
}

#[test]
fn vault_paths_resolve_symlinks_and_missing_descendants_without_creating_state() {
    let fixture = Fixture::new();
    let alias = fixture.0.join("synthetic-alias");
    std::os::unix::fs::symlink(fixture.0.join(".build"), &alias).unwrap();
    let value = alias.join("secrets/missing");
    let error = configured(
        &fixture.0,
        "prod",
        "BRIDGE_TASKS_DIR",
        value.to_str().unwrap(),
    )
    .unwrap_err();
    assert!(error.contains("vault"));
    assert!(!fixture.0.join(".build").exists());
}

#[test]
fn vault_paths_allow_disjoint_roots_and_prefix_siblings() {
    let fixture = Fixture::new();
    for channel in ["prod", "dev"] {
        let config = configured(
            &fixture.0,
            channel,
            "BRIDGE_TASKS_DIR",
            fixture.0.join("safe-tasks").to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(config.tasks_dir, fixture.0.join("safe-tasks"));
        let safe = config.state_root.join("secrets-fixtures");
        for key in [
            "BRIDGE_TASKS_DIR",
            "BRIDGE_PROJECTS_DIR",
            "BRIDGE_WORKTREES",
            "BRIDGE_REPO",
            "BRIDGE_PROJECTS",
        ] {
            configured(&fixture.0, channel, key, safe.to_str().unwrap()).unwrap();
        }
    }
}

#[test]
fn vault_paths_refuse_legacy_update_job_overlaps_for_both_device_roots() {
    let fixture = Fixture::new();
    for root in [
        fixture.0.join(".build"),
        fixture.0.join(".local/state/build-dev"),
    ] {
        for tasks in [&fixture.0, &root, &root.join("secrets/old-job")] {
            assert!(vault::validate_update_tasks(&fixture.0, tasks).is_err());
        }
    }
    vault::validate_update_tasks(&fixture.0, &fixture.0.join("safe-tasks")).unwrap();
}
