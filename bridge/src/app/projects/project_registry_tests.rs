//! Project registry ownership and publication invariants.

use super::project_registry::{ProjectCandidate, ProjectId};
use super::{Project, ProjectRegistry};
use crate::orchestrator::{Agent, Orchestrator};
use crate::pty::HarnessSpec;
use crate::templates::Templates;
use std::path::{Path, PathBuf};

#[test]
fn source_path_preserves_exact_checkout_root_for_git_directory_and_file_markers() {
    for marker_is_file in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let checkout = root
            .path()
            .join(if marker_is_file { "worktree" } else { "repo" });
        let nested = checkout.join("packages/app");
        std::fs::create_dir_all(&nested).unwrap();
        if marker_is_file {
            std::fs::write(checkout.join(".git"), "gitdir: ../metadata\n").unwrap();
        } else {
            std::fs::create_dir(checkout.join(".git")).unwrap();
        }
        assert_eq!(super::canonical_source_path(&checkout).unwrap(), checkout);
        assert_eq!(super::canonical_source_path(&nested).unwrap(), nested);
    }
}

#[test]
fn mount_names_are_safe_segments() {
    assert_eq!(super::safe_mount_name("API service"), "API-service");
    assert_eq!(super::safe_mount_name("../"), "source");
}

fn candidate(
    registry: &ProjectRegistry,
    path: PathBuf,
    build: impl FnOnce(ProjectId, &Path, &str) -> Project,
) -> ProjectCandidate {
    registry.candidate(path, "main".to_string(), build)
}

#[test]
fn publication_is_monotonic_in_registration_order() {
    let root = tempfile::tempdir().unwrap();
    let mut registry = ProjectRegistry::new();
    let ids = ["one", "two", "three"].map(|name| {
        let path = root.path().join(name);
        std::fs::create_dir(&path).unwrap();
        let prospective = candidate(&registry, path, project_fixture);
        registry.publish(prospective).into_string()
    });
    assert_eq!(ids, ["proj-1", "proj-2", "proj-3"]);
    assert_eq!(registry.ids().collect::<Vec<_>>(), ids);
}

#[test]
fn dropping_a_candidate_leaves_every_published_view_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let registry = ProjectRegistry::new();

    let failed_write = candidate(&registry, repo.clone(), project_fixture);
    drop(failed_write);
    assert_eq!(registry.iter().count(), 0);
    assert!(registry.find_by_canonical_path(&repo).is_none());
    assert_eq!(
        candidate(&registry, repo, project_fixture).project().id,
        "proj-1"
    );
}

#[test]
fn retry_after_failed_persistence_publishes_once() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let other = root.path().join("other");
    std::fs::create_dir(&repo).unwrap();
    std::fs::create_dir(&other).unwrap();
    let mut registry = ProjectRegistry::new();

    drop(candidate(&registry, repo.clone(), project_fixture));
    let retry = candidate(&registry, repo, project_fixture);
    assert_eq!(registry.publish(retry).as_str(), "proj-1");
    let later = candidate(&registry, other, project_fixture);
    assert_eq!(registry.publish(later).as_str(), "proj-2");
}

#[test]
fn path_prefers_live_project_then_retained_text_then_empty() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut registry = ProjectRegistry::new();
    let id = registry
        .publish(candidate(&registry, repo.clone(), project_fixture))
        .into_string();
    registry.retain_entity_path("entity".into(), "stored/../verbatim".into());
    registry.bind_entity("entity".into(), id);
    assert_eq!(
        registry.project_path_for("entity"),
        repo.display().to_string()
    );

    registry.bind_entity("entity".into(), "missing-project".into());
    assert_eq!(registry.project_path_for("entity"), "stored/../verbatim");
    assert_eq!(registry.project_path_for("unknown"), "");
}

#[test]
fn unbind_removes_both_live_binding_and_retained_path() {
    let mut registry = ProjectRegistry::new();
    registry.bind_entity("entity".into(), "proj-1".into());
    registry.retain_entity_path("entity".into(), "/missing/repo".into());
    registry.unbind_entity("entity");
    assert_eq!(registry.project_id_of("entity"), None);
    assert_eq!(registry.project_path_for("entity"), "");
}

#[test]
fn missing_repository_recovery_keeps_verbatim_path_without_a_live_binding() {
    let mut registry = ProjectRegistry::new();
    registry.retain_entity_path("run-1".into(), "~/gone/../repo".into());
    assert_eq!(registry.project_id_of("run-1"), None);
    assert_eq!(registry.project_path_for("run-1"), "~/gone/../repo");
}

/// Construct the real capability-owned Project without starting a session.
fn project_fixture(id: ProjectId, path: &Path, base: &str) -> Project {
    let id = id.into_string();
    Project {
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project")
            .to_string(),
        repo_path: path.to_path_buf(),
        base_branch: base.to_string(),
        is_git: true,
        sources: vec![],
        orch: Orchestrator::new(
            path,
            path.join(".registry-test-worktrees").join(&id),
            Agent::Warm(HarnessSpec::new("true")),
            Templates::default(),
            PathBuf::from("/bin/true"),
        ),
        id,
        isolation: None,
    }
}
