use super::*;

#[test]
fn store_free_thread_read_context_keeps_an_immutable_bounded_history() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    state.store = None;
    pagination::run_with_long_conversation(&mut state, "run-read", 250);
    let params = json!({ "entity_id": "run-read", "limit": 3 });
    let expected = state.thread_page(&params).unwrap();
    let context = state.thread_read_context();
    state.runs.remove("run-read");

    assert_eq!(context.read(None, "thread.page", &params).unwrap(), expected);
    assert_eq!(expected["items"].as_array().unwrap().len(), 3);
    assert_eq!(expected["thread_total"], 250);
    assert_eq!(
        state.thread_read_context().read(None, "thread.page", &params),
        Err("unknown id".into())
    );
}
