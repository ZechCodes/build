use super::*;

fn published_run_stat(state: &mut AppState, run: &str) -> Value {
    let reply = state.handle(req("board.list", json!({})));
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_id"] == run)
        .unwrap()["stat"]
        .clone()
}

#[test]
fn changed_run_stat_is_published_after_install_before_snapshot_reads() {
    let (home, repository) = init_repo();
    let mut state = qa_state(&repository, home.path());
    let run = adopted_run(&mut state, &repository, home.path(), "stat-publication");
    let DiffCacheEntry::RunStat { mut stat, .. } =
        state.run_stat_refresh(&run).unwrap().compute(None).unwrap()
    else {
        panic!("run refresh produces a run stat")
    };
    stat["files_changed"] = json!(1);
    state.store_run_stat(run.clone(), stat.clone(), std::time::Instant::now());
    state.publish_snapshot_projection();
    assert_eq!(
        published_run_stat(&mut state, &run),
        stat,
        "the initial snapshot is the control"
    );

    stat["files_changed"] = json!(2);
    state.store_run_stat(run.clone(), stat.clone(), std::time::Instant::now());
    assert_eq!(
        state.board.diff().run_stat(&run).unwrap().value,
        stat,
        "the producer installed its new observation"
    );
    assert_eq!(
        published_run_stat(&mut state, &run),
        stat,
        "a completed producer update must publish before a snapshot can refetch it"
    );
}
