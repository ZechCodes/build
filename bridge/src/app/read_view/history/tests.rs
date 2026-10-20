use super::*;

#[test]
fn shared_history_preserves_full_page_and_mutation_cursor_shapes() {
    let mut thread = Thread::for_agent("agent");
    let _message = thread.post_user("first", None, "now");
    thread.post_user("second", None, "later");
    let saved = SharedHistory::warm(&thread, None).unwrap();
    assert_eq!(saved.render(&json!({})), thread.wire_value());
    assert_eq!(
        saved.render(&json!({"thread_limit":1})),
        thread.wire_value_page(None, 1)
    );
    let cursor = thread.last_sequence();
    thread.read_unread("seen");
    let updated = saved.updated(&thread);
    assert_eq!(
        updated.render(&json!({"thread_after_sequence":cursor})),
        thread.wire_value_after(cursor)
    );
    assert_eq!(
        saved.render(&json!({}))["items"][0]["data"]["seen_at"],
        Value::Null
    );
}

#[test]
fn unchanged_items_are_shared_and_reset_drops_old_history() {
    let mut thread = Thread::for_agent("agent");
    for _ in 0..1024 {
        thread.post_user("words", None, "now");
    }
    let saved = SharedHistory::warm(&thread, None).unwrap();
    thread.post_user("new", None, "later");
    let next = saved.updated(&thread);
    assert!(Arc::ptr_eq(saved.item(1).unwrap(), next.item(1).unwrap()));
    assert_eq!(
        saved.render(&json!({}))["items"].as_array().unwrap().len(),
        1024
    );
    let mut reset = Thread::for_agent("agent");
    reset.generation_revision = 1;
    reset.post_user("fresh", None, "after reset");
    let next = next.updated(&reset);
    assert_eq!(next.render(&json!({})), reset.wire_value());
    assert_eq!(next.unread_since(0), reset.unread_since(0));
    assert_eq!(next.updated(&thread).render(&json!({})), reset.wire_value());
}

#[test]
fn delayed_same_generation_observation_cannot_regress_metadata() {
    let mut thread = Thread::for_agent("agent");
    thread.post_user("old", None, "first");
    let old = thread.clone();
    thread.post_user("new", None, "second");
    let saved = SharedHistory::warm(&thread, None).unwrap();
    let delayed = saved.updated(&old);
    assert_eq!(
        delayed.render(&json!({"thread_after_sequence":0})),
        thread.wire_value_after(0)
    );
}

#[test]
fn initial_warm_reads_older_mutations_once_and_later_reads_need_no_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut agent = crate::agent::Agent::new("agent", "owner", Default::default(), 0, "now");
    let thread = &mut agent.thread;
    for _ in 0..300 {
        thread.post_user("words", None, "now");
    }
    let cursor = thread.last_sequence();
    thread.read_unread("seen");
    let full = thread.clone();
    let mut run: crate::store::PersistedRun = serde_json::from_value(json!({
        "id":"owner", "goal":"test", "project_path":"/temporary", "base_branch":"main",
        "state":"Created", "branch":"test", "worktree_name":"test", "worktree_path":"/temporary",
        "last_summary":null, "created_at":"now", "updated_at":"now"
    }))
    .unwrap();
    run.agents = vec![agent];
    store.save_run(&run).unwrap();
    let mut tail = full.clone();
    tail.adopt_stored_tail(full.items[298..].to_vec(), 298, full.last_sequence());
    let warmed = SharedHistory::warm(&tail, Some(&store)).unwrap();
    drop(store);
    assert_eq!(
        warmed.render(&json!({"thread_after_sequence":cursor})),
        full.wire_value_after(cursor)
    );
    assert_eq!(warmed.render(&json!({})), full.wire_value());
    assert_eq!(warmed.unread_since(0), full.unread_since(0));
}

#[test]
fn pages_keep_exact_digests_and_original_item_budgets() {
    let mut thread = Thread::for_agent("agent");
    thread.post_user("opening", None, "first");
    for _ in 0..300 {
        thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            Some("Read a.rs".into()),
            None,
            None,
            "work",
        );
    }
    thread.post_agent("answer", None, "last");
    let saved = SharedHistory::warm(&thread, None).unwrap();
    for limit in [1, 2, 20] {
        assert_eq!(
            saved.render(&json!({"thread_limit":limit})),
            thread.wire_value_page(None, limit)
        );
    }
}

#[test]
fn truncated_store_free_history_reports_unavailable_instead_of_empty() {
    let mut thread = Thread::for_agent("agent");
    thread.post_user("resident", None, "now");
    thread.adopt_stored_tail(thread.items.clone(), 20, 20);
    assert_eq!(
        SharedHistory::warm(&thread, None).err().as_deref(),
        Some("this conversation's history is not stored")
    );
}

#[test]
fn removal_keeps_the_previous_snapshot_and_drops_only_the_removed_row() {
    let mut thread = Thread::for_agent("agent");
    thread.post_user("first", None, "now");
    thread.post_user("second", None, "now");
    let saved = SharedHistory::warm(&thread, None).unwrap();
    thread.items.remove(0);
    let next = saved.updated(&thread);
    assert_eq!(next.render(&json!({})), thread.wire_value());
    assert_eq!(
        saved.render(&json!({}))["items"].as_array().unwrap().len(),
        2
    );
    assert!(Arc::ptr_eq(saved.item(2).unwrap(), next.item(2).unwrap()));
}

#[test]
fn withdrawing_the_newest_item_does_not_look_like_a_stale_projection() {
    let mut thread = Thread::for_agent("agent");
    thread.post_user("first", None, "now");
    thread.post_user("withdrawn", None, "now");
    let saved = SharedHistory::warm(&thread, None).unwrap();
    thread.items.pop();
    assert_eq!(
        saved.updated(&thread).render(&json!({})),
        thread.wire_value()
    );
}

#[test]
fn committed_historical_rows_cannot_cross_a_reset_generation() {
    let mut old = Thread::for_agent("agent");
    old.post_user("old message", None, "before");
    old.read_unread("seen");
    let delta = crate::store::CommittedThreadItems {
        conversation_id: old.agent.id.clone(),
        thread_id: old.id.clone(),
        generation_revision: old.generation_revision,
        sequence: old.last_sequence(),
        items: old.items,
    };
    let mut reset = Thread::for_agent("agent");
    reset.generation_revision = 1;
    let history = SharedHistory::warm(&reset, None).unwrap();
    assert!(history.with_committed_items(&delta).is_none());
    assert_eq!(history.render(&json!({})), reset.wire_value());
}
