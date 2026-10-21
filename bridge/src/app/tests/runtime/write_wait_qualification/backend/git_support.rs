//! OFFSOURCE: native setup only, before the immutable trial schedule starts.
use super::super::common_requests::{ActiveTarget, CommonTargets, ProjectTarget, RootGitRefs};
use crate::app::tests::runtime::write_wait_qualification::fixture::QualificationFixture;
use std::collections::BTreeSet;

/// Four local bare remotes and sixteen unique branch refs are support objects,
/// not app WorkspaceRegistry residents, semantic writes or provider endpoints.
pub(super) fn prepare(fixture: &QualificationFixture) -> Result<CommonTargets, String> {
    for project in &fixture.projects {
        let parent = project
            .repository
            .parent()
            .ok_or("private repo has no parent")?;
        let path = parent.join(format!("qual-local-origin-{}.git", project.project_id));
        let repo = git2::Repository::open(&project.repository).map_err(|e| e.to_string())?;
        let bare = git2::Repository::init_bare(&path).map_err(|e| e.to_string())?;
        let main = repo
            .find_reference("refs/heads/main")
            .map_err(|e| e.to_string())?
            .target()
            .ok_or("main is symbolic")?;
        drop(bare);
        if repo.find_remote("origin").is_ok() {
            repo.remote_delete("origin").map_err(|e| e.to_string())?;
        }
        let mut remote = repo
            .remote("origin", path.to_str().ok_or("private origin is not UTF8")?)
            .map_err(|e| e.to_string())?;
        remote
            .push(&["refs/heads/main:refs/heads/main"], None)
            .map_err(|e| e.to_string())?;
        remote
            .fetch(&["refs/heads/main:refs/remotes/origin/main"], None, None)
            .map_err(|e| e.to_string())?;
        repo.find_branch("main", git2::BranchType::Local)
            .map_err(|e| e.to_string())?
            .set_upstream(Some("origin/main"))
            .map_err(|e| e.to_string())?;
        let fetched = git2::Repository::open_bare(&path)
            .map_err(|e| e.to_string())?
            .refname_to_id("refs/heads/main")
            .map_err(|e| e.to_string())?;
        if main != fetched {
            return Err("private origin did not retain real main commit".into());
        }
    }
    let active = fixture
        .active_workspaces
        .iter()
        .map(|w| ActiveTarget {
            owner: w.owner.clone(),
            agent: w.agent_id.clone(),
            thread: w.thread_id.clone(),
            conversation: w.conversation_id.clone(),
            workspace: w.workspace_id.clone(),
            source: w.source_id.clone(),
            path: w.directory.clone(),
            project: w.project,
        })
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| "exact eight active roots")?;
    let projects = fixture
        .projects
        .iter()
        .map(|p| ProjectTarget {
            project_id: p.project_id.clone(),
            ordinary_tasks: [p.tasks[0].clone(), p.tasks[1].clone()],
        })
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| "exact four projects")?;
    let mut refs = Vec::with_capacity(8);
    let mut bodies = Vec::with_capacity(8);
    for (index, workspace) in fixture.active_workspaces.iter().enumerate() {
        let repo = git2::Repository::open(&workspace.directory).map_err(|e| e.to_string())?;
        let head = repo.head().map_err(|e| e.to_string())?;
        let commit = head.peel_to_commit().map_err(|e| e.to_string())?.id();
        let initial_head_ref = head.name().ok_or("initial HEAD not UTF8")?.to_string();
        let full_refs = [
            format!("refs/heads/qual-active-{index}-a"),
            format!("refs/heads/qual-active-{index}-b"),
        ];
        for name in &full_refs {
            repo.reference(name, commit, false, "qualification support before T0")
                .map_err(|e| e.to_string())?;
        }
        refs.push(RootGitRefs {
            full_refs,
            initial_commit: commit.to_string(),
            initial_head_ref,
        });
        bodies.push(
            std::fs::read(workspace.directory.join("qualified-editor.txt"))
                .map_err(|e| e.to_string())?,
        );
    }
    let reserved_read_tasks = fixture
        .projects
        .iter()
        .flat_map(|p| p.tasks.iter().rev().take(2).cloned())
        .collect::<BTreeSet<_>>();
    Ok(CommonTargets {
        active,
        projects,
        git: refs.try_into().map_err(|_| "exact eight ref pairs")?,
        initial_editor_bodies: bodies.try_into().map_err(|_| "exact eight editor bodies")?,
        reserved_read_tasks,
    })
}
