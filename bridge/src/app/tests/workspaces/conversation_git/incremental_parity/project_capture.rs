//! One candidate-local native capture must feed shape proof and project refresh.
use super::*;
use crate::app::snapshot_projection::work_counts::project_capture as work;
use crate::compaction::CompactionScope;

fn warm_projects(app: &mut AppState) {
    let rows = app
        .capture_project_snapshot_observations()
        .into_iter()
        .map(|row| {
            let mut value = row.render();
            for source in value["sources"].as_array_mut().unwrap() {
                source["remote"] = json!(format!("observed-{}", source["id"].as_str().unwrap()));
            }
            (row.project_id.clone(), value)
        })
        .collect();
    app.install_project_snapshot_observations(rows);
    app.publish_all_snapshot_projection();
}

fn project_fixture() -> (tempfile::TempDir, AppState, String) {
    let (tmp, mut app) = fixture();
    let other = tmp.path().join("other-project");
    std::fs::create_dir_all(&other).unwrap();
    app.add_project(other, "main".into());
    let project = app.projects.project_id_of("run-00").unwrap().to_owned();
    app.config_path = Some(tmp.path().join("project-config.json"));
    warm_projects(&mut app);
    app.persist();
    (tmp, app, project)
}

fn row(root: &ReadView, project: &str) -> Value {
    root.get_owned("project.list", "").unwrap()["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["project_id"] == project)
        .unwrap()
        .clone()
}

fn assert_reference(app: &AppState, root: &ReadView, requests: &[FactsRequest]) {
    let before = image(root, requests);
    // Generic Full preserves the same named-agent/history admission as the
    // owner/task publication, including unrelated incidental project metadata.
    app.publish_snapshot_projection();
    assert_eq!(before, image(&app.read_view.load(), requests));
    assert_eq!(before, image(root, requests));
}

fn assert_fast(app: &AppState, owner: &str) {
    let _guard = app.begin_snapshot_ownership();
    assert!(app
        .prepare_incremental_owner_snapshot(
            SnapshotDelta::new(app.read_view.next_version()),
            &app.read_view.load(),
            &BTreeSet::from([owner.into()]),
        )
        .is_some());
}

fn assert_fallback(app: &AppState, previous: &ReadView) {
    let _guard = app.begin_snapshot_ownership();
    assert!(app
        .prepare_incremental_owner_snapshot(
            SnapshotDelta::new(app.read_view.next_version()),
            previous,
            &BTreeSet::from(["run-00".into()]),
        )
        .is_none());
}

fn project_keys(app: &AppState, visits: usize) -> BTreeMap<String, usize> {
    app.projects
        .iter()
        .map(|project| (project.id.clone(), visits))
        .collect()
}

#[test]
fn incremental_project_capture_owner_and_task_capture_each_native_project_once() {
    let (_tmp, mut app, project) = project_fixture();
    let requests = requests(&app);
    let old = app.read_view.load();
    let retained = image(&old, &requests);
    let (_, positive) = work::measure(|| app.project_list_rows());
    assert_eq!(positive.captures, 1, "actual source positive");
    assert_eq!(positive.projects, project_keys(&app, 1));

    app.runs.get_mut("run-00").unwrap().run.goal = "project capture owner control".into();
    app.session_summaries.insert(
        project.clone(),
        crate::session_summary::SessionSummary::default().updated(777),
    );
    assert_fast(&app, "run-00");
    let (_, owner) = work::measure(|| app.publish_owner_snapshot_projection("run-00"));
    let owner_root = app.read_view.load();
    assert_eq!(row(&owner_root, &project)["last_activity_ms"], 777);
    assert_ne!(row(&old, &project)["last_activity_ms"], 777);
    assert!(
        format!("{:?}", image(&owner_root, &requests)).contains("project capture owner control")
    );
    assert_reference(&app, &owner_root, &requests);
    assert_eq!(retained, image(&old, &requests));

    app.install_snapshot_watched_task(&project, "project-capture-task", Some(json!({
        "kind":"tracker_task", "task_id":"project-capture-task", "title":"captured task control", "anchor":CLOCK,
    })));
    let (_, task) = work::measure(|| app.publish_task_snapshot_projection());
    let task_root = app.read_view.load();
    assert!(task_root.get_owned("board.list", "").unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["task_id"] == "project-capture-task"
            && row["title"] == "captured task control"));
    assert_eq!(row(&task_root, &project)["last_activity_ms"], 777);
    assert_reference(&app, &task_root, &requests);

    // Both producer semantics and complete native source coverage precede RED.
    for (label, count) in [("owner", &owner), ("task", &task)] {
        eprintln!("project capture {label}: {count:?}");
        assert!(count.captures > 0);
        assert_eq!(count.projects, project_keys(&app, count.captures));
    }
    for count in [owner, task] {
        assert_eq!(count.captures, 1, "duplicate native project-list capture");
        assert_eq!(count.projects, project_keys(&app, 1));
    }
}

#[test]
fn incremental_project_capture_keeps_alias_source_order_and_policy_fallbacks() {
    let (_tmp, mut app, project) = project_fixture();
    let canonical = app.runs["run-00"].agents.iter().next().unwrap().id.clone();
    let old = app.read_view.load();
    app.runs
        .get_mut("run-01")
        .unwrap()
        .agents
        .iter_mut()
        .next()
        .unwrap()
        .bind_conversation(&canonical);
    assert_fallback(&app, &old);
    app.publish_owner_snapshot_projection("run-00");
    let requests = requests(&app);
    assert_reference(&app, &app.read_view.load(), &requests);

    let before_order = app.read_view.load();
    let mut added = app.projects.get(&project).unwrap().sources[0].clone();
    added.id = "source-second".into();
    added.name = "second native source".into();
    let index = app.projects.iter().position(|p| p.id == project).unwrap();
    app.projects.at_mut(index).sources.push(added);
    app.projects.at_mut(index).sources.reverse();
    assert_fallback(&app, &before_order);
    warm_projects(&mut app);
    let ordered = app.read_view.load();
    assert_eq!(row(&ordered, &project)["sources"][0]["id"], "source-second");
    assert_eq!(row(&ordered, &project)["remote"], "observed-source-second");
    assert_eq!(
        row(&ordered, &project)["path"],
        app.projects
            .get(&project)
            .unwrap()
            .repo_path
            .display()
            .to_string()
    );
    assert_reference(&app, &ordered, &requests);

    let old = app.read_view.load();
    let retained = image(&old, &requests);
    app.patch_compaction_thresholds(
        CompactionScope::Project(project.clone()),
        &json!({"claude":123_456}),
    )
    .unwrap();
    assert_fallback(&app, &old);
    let next = app.read_view.load();
    assert_eq!(
        row(&next, &project)["compaction"]["effective"]["claude"]["tokens"],
        123_456
    );
    assert_reference(&app, &next, &requests);
    assert_eq!(retained, image(&old, &requests));
}

#[test]
fn incremental_project_capture_failed_config_never_publishes_candidate_rows() {
    let (tmp, mut app, project) = project_fixture();
    let requests = requests(&app);
    for failure in [
        crate::app::ConfigPersistStep::Write,
        crate::app::ConfigPersistStep::Rename,
    ] {
        let old = app.read_view.load();
        let retained = image(&old, &requests);
        let disk = std::fs::read(tmp.path().join("project-config.json")).unwrap();
        app.config_persist_failure = Some(failure);
        assert!(app
            .patch_compaction_thresholds(
                CompactionScope::Project(project.clone()),
                &json!({"claude":345_678})
            )
            .is_err());
        app.config_persist_failure = None;
        assert!(Arc::ptr_eq(&old, &app.read_view.load()));
        assert_eq!(retained, image(&app.read_view.load(), &requests));
        assert_eq!(
            disk,
            std::fs::read(tmp.path().join("project-config.json")).unwrap()
        );
        app.runs.get_mut("run-00").unwrap().run.goal = "committed policy retry".into();
        assert_fast(&app, "run-00");
        app.publish_owner_snapshot_projection("run-00");
        let next = app.read_view.load();
        assert_eq!(
            row(&next, &project)["compaction"],
            row(&old, &project)["compaction"]
        );
        assert_reference(&app, &next, &requests);
        assert_eq!(retained, image(&old, &requests));
    }
}

#[test]
fn incremental_project_capture_failed_owner_and_stale_delta_leave_accepted_rows() {
    let (_tmp, mut app, project) = project_fixture();
    let requests = requests(&app);
    let old = app.read_view.load();
    let retained = image(&old, &requests);
    let mut failed = app.take_run("run-00").unwrap();
    failed.run.goal = "UNCOMMITTED project capture".into();
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app.finish_run_mutation("run-00".into(), failed).is_err());
    assert!(Arc::ptr_eq(&old, &app.read_view.load()));
    assert_eq!(retained, image(&app.read_view.load(), &requests));
    app.publish_owner_snapshot_projection("run-02"); // Accept restored pending-owner retry.

    let accepted = app.read_view.load();
    let before = image(&accepted, &requests);
    let prior_summary = app.session_summaries[&project];
    app.session_summaries.insert(
        project.clone(),
        crate::session_summary::SessionSummary::default().updated(888),
    );
    assert_eq!(app.session_summary(&project).last_activity_ms, Some(888));
    let stale = app.prepare_owner_snapshot_projection("run-00").unwrap();
    app.session_summaries.insert(project.clone(), prior_summary);
    let bump = SnapshotDelta::new(app.read_view.next_version())
        .set_global(accepted.global.as_ref().unwrap().clone());
    assert!(app.read_view.publish_delta(bump));
    let advanced = app.read_view.load();
    app.publish_prepared_snapshot_projection(stale);
    assert!(Arc::ptr_eq(&advanced, &app.read_view.load()));
    assert_eq!(before, image(&advanced, &requests));
    app.publish_owner_snapshot_projection("run-02");
    assert_ne!(
        row(&app.read_view.load(), &project)["last_activity_ms"],
        888
    );
    assert_reference(&app, &app.read_view.load(), &requests);
    assert_eq!(retained, image(&old, &requests));
}
