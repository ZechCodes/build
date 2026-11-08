use super::*;
use crate::app::facts::entity_facts;
use crate::changes::{project_source_entity_id, FactsRequest};
use crate::git_fixture::{configure_repo, git_in, init_repo_named};
use std::path::Path;

struct SharedFixture {
    _root: tempfile::TempDir,
    upstream: PathBuf,
    checkout: PathBuf,
    projects: [String; 2],
    app: AppState,
}

fn fixture() -> SharedFixture {
    let root = tempfile::tempdir().unwrap();
    let upstream = init_repo_named(root.path(), "upstream");
    let checkout = root.path().join("shared");
    git_in(
        root.path(),
        &["clone", "-q", upstream.to_str().unwrap(), "shared"],
    );
    configure_repo(&checkout);
    let context = crate::harness::HarnessContext::resolved(
        root.path().join("mcp.sock"),
        root.path().join("state"),
    )
    .unwrap();
    let mut app =
        AppState::new_unrooted_configured(root.path().join("worktrees"), "main", true, context);
    app.store = Some(crate::store::Store::new(root.path().join("records")).unwrap());
    let projects = ["one", "two"].map(|name| {
        let home = init_repo_named(root.path(), name);
        let project = app.add_project(home, "main".into());
        let mut sources = app.sources_for(&project).unwrap();
        sources.push(super::super::ProjectSource::added(
            "source-2".into(),
            "shared".into(),
            "shared".into(),
            checkout.clone(),
            true,
            "main".into(),
        ));
        assert!(app.projects.set_sources(&project, sources));
        project
    });
    SharedFixture {
        _root: root,
        upstream,
        checkout,
        projects,
        app,
    }
}

fn advance(repo: &Path, name: &str) {
    std::fs::write(repo.join(name), name).unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", name]);
}

fn requests(projects: &[String; 2], checkout: &Path) -> Vec<FactsRequest> {
    projects
        .iter()
        .map(|project| FactsRequest {
            entity_id: project_source_entity_id(project, "source-2", checkout.to_str().unwrap()),
            git: true,
            ..FactsRequest::default()
        })
        .collect()
}

fn warm_sources(state: &Arc<Mutex<AppState>>, requests: &[FactsRequest]) {
    let handle = Arc::clone(&state.lock().unwrap().facts_handle);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if entity_facts(&handle, requests)
            .iter()
            .all(|fact| fact.status_key.is_some())
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "source observations were not published"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn shared_repository_mutation_invalidates_every_referencing_project() {
    let SharedFixture {
        _root: root,
        upstream,
        checkout,
        projects,
        mut app,
    } = fixture();
    // Multi-source projects have no automatic legacy primary alias. Adopt
    // this original checkout explicitly to exercise the workspace selector.
    let alias = app.workspaces.adopt_git_root(
        &projects[0],
        "adopted-shared".into(),
        "Shared checkout".into(),
        checkout.clone(),
        "source-2".into(),
    );
    let state = app.shared();
    let requests = requests(&projects, &checkout);
    warm_sources(&state, &requests);
    advance(&upstream, "incoming.txt");
    let subject = {
        let app = state.lock().unwrap();
        let source = &app.projects.get(&projects[0]).unwrap().sources[1];
        subject(
            &projects[0],
            source,
            Fetch::Within(SERVICE_FETCH_DEADLINE),
            SyncOrigin::Service,
        )
    };
    let synced = sync_for_the_service(subject, &state).unwrap();
    assert!(synced.report.fetched);
    assert!(matches!(
        synced.report.outcome,
        SyncOutcome::FastForwarded { .. }
    ));
    let mut app = state.lock().unwrap();
    app.settle_source_syncs(vec![synced], 1_000);
    assert_source_facts_invalidated(&app, &requests, &projects);
    drop(app);
    for (index, (scope, fails)) in [
        (
            json!({"project_id": projects[0], "source_id":"source-2", "source_path":checkout}),
            false,
        ),
        (
            json!({"workspace_id":alias.id, "source_id":"source-2"}),
            false,
        ),
        (
            json!({"project_id": projects[0], "source_id":"source-2", "source_path":checkout}),
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        warm_sources(&state, &requests);
        advance(&upstream, &format!("fetched-{index}.txt"));
        if fails {
            let missing = root.path().join("missing-upstream.git");
            git_in(
                &checkout,
                &["remote", "set-url", "origin", missing.to_str().unwrap()],
            );
        }
        let git = {
            let mut app = state.lock().unwrap();
            app.git_fetch(&scope).unwrap();
            let Some(crate::app::DeferredWork::Git(git)) = app.deferred_work.take() else {
                panic!("fetch must run off lock");
            };
            git
        };
        let result = git.run();
        assert_eq!(result.is_err(), fails, "{result:?}");
        let mut app = state.lock().unwrap();
        let settled = app.apply_git(&git, result);
        assert_eq!(settled.is_err(), fails, "{settled:?}");
        assert_source_facts_invalidated(&app, &requests, &projects);
    }
}

fn assert_source_facts_invalidated(
    app: &AppState,
    requests: &[FactsRequest],
    projects: &[String; 2],
) {
    let facts = entity_facts(&app.facts_handle, requests);
    let rows = app.read_view.load().get_owned("project.list", "").unwrap();
    for (project, fact) in projects.iter().zip(facts) {
        assert!(
            fact.status_key.is_none(),
            "repository mutation kept stale source facts for {project}"
        );
        let source = &app.projects.get(project).unwrap().sources[1];
        let row = rows["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["project_id"] == *project)
            .unwrap();
        assert_eq!(
            row["sources"][1]["remote"],
            json!(source.origin()),
            "repository mutation kept stale source remote for {project}"
        );
    }
}

#[test]
fn shared_directory_different_bases_keep_separate_sync_status() {
    let mut fixture = fixture();
    git_in(&fixture.checkout, &["branch", "topic"]);
    let mut sources = fixture.app.sources_for(&fixture.projects[1]).unwrap();
    sources[1].base_branch = "topic".into();
    assert!(fixture
        .app
        .projects
        .set_sources(&fixture.projects[1], sources));
    let state = fixture.app.shared();
    for (index, project) in fixture.projects.iter().enumerate() {
        let captured = {
            let app = state.lock().unwrap();
            subject(
                project,
                &app.projects.get(project).unwrap().sources[1],
                Fetch::Within(SERVICE_FETCH_DEADLINE),
                SyncOrigin::Service,
            )
        };
        let synced = sync_for_the_service(captured, &state).unwrap();
        state
            .lock()
            .unwrap()
            .settle_source_syncs(vec![synced], 1_000 + index as i64);
    }
    let mut app = state.lock().unwrap();
    let statuses: Vec<_> = fixture
        .projects
        .iter()
        .map(|project| {
            app.projects.get(project).unwrap().sources[1]
                .sync_status
                .clone()
                .unwrap()
        })
        .collect();
    assert_eq!(statuses[0].state, SyncState::Synced);
    assert_ne!(
        statuses[1].state,
        SyncState::Synced,
        "topic has no upstream branch"
    );
    assert_eq!(statuses[0].last_attempt_ms, 1_000);
    assert_eq!(statuses[1].last_attempt_ms, 1_001);
    app.commit_project_configuration(&fixture.projects[0], |projects| {
        projects
            .source_mut(&fixture.projects[0], "source-2")
            .unwrap()
            .name = "Renamed shared folder".into();
        Ok(())
    })
    .unwrap();
    for (project, expected) in fixture.projects.iter().zip(&statuses) {
        assert_eq!(
            app.projects.get(project).unwrap().sources[1]
                .sync_status
                .as_ref(),
            Some(expected)
        );
    }
    let persisted = app
        .store
        .as_ref()
        .unwrap()
        .load_source_syncs::<SyncStatus>()
        .unwrap();
    assert_eq!(
        persisted[&status_key(&fixture.projects[0], "source-2")],
        statuses[0]
    );
    assert_eq!(
        persisted[&status_key(&fixture.projects[1], "source-2")],
        statuses[1]
    );
    for project in &fixture.projects {
        app.projects
            .source_mut(project, "source-2")
            .unwrap()
            .sync_status = None;
    }
    app.restore_source_syncs().unwrap();
    for (project, expected) in fixture.projects.iter().zip(statuses) {
        assert_eq!(
            app.projects.get(project).unwrap().sources[1]
                .sync_status
                .as_ref(),
            Some(&expected)
        );
    }
}
