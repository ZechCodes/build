//! Ordered query validity must use canonical bytes, never a display prefix.
use super::*;
use crate::store::TaskQueryOrder;
use crate::tracker::{TaskEvent, TaskEventKind};

const GOOD: &str = "2026-10-01T00:00:00Z";
fn domain(store: &Store, project: &str, order: TaskQueryOrder, done: bool, want: Option<&str>) {
    assert_eq!(
        store
            .task_query_unsupported_domain(project, order, done)
            .unwrap()
            .as_deref(),
        want
    );
}
#[test]
fn query_source_timestamp_bytes_reject_nuls_and_overlong_valid_prefixes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    for (n, bad) in [
        format!("{GOOD}\0junk"),
        format!("\0{GOOD}"),
        format!("{GOOD}{}junk", " ".repeat(80)),
    ]
    .into_iter()
    .enumerate()
    {
        for field in [
            "updated",
            "created",
            "anchor_updated",
            "done",
            "done_created",
        ] {
            let project = format!("/raw-{n}-{field}");
            let mut draft = Task::drafted(&project, "raw bytes", Actor::User, GOOD);
            match field {
                "updated" => draft.updated_at = bad.clone(),
                "created" => draft.created_at = bad.clone(),
                "anchor_updated" => {
                    draft.created_at = "".into();
                    draft.updated_at = bad.clone();
                }
                "done" => {
                    draft.status = "done".into();
                    draft.done_at = Some(bad.clone());
                }
                "done_created" => {
                    draft.status = "done".into();
                    draft.created_at = bad.clone();
                }
                _ => unreachable!(),
            }
            let task = store.create_tracker_task(draft, &[]).unwrap();
            let (order, done, want) = match field {
                "updated" => (TaskQueryOrder::Updated, false, "updated_at"),
                "created" | "anchor_updated" => (TaskQueryOrder::Anchor, false, "anchor_fallback"),
                _ => (TaskQueryOrder::Number, true, "done_at"),
            };
            domain(&store, &project, order, done, Some(want));
            let canonical = store.load_tracker_task(&task.id).unwrap().unwrap();
            let actual = match field {
                "updated" | "anchor_updated" => canonical.updated_at,
                "created" | "done_created" => canonical.created_at,
                _ => canonical.done_at.unwrap(),
            };
            assert_eq!(
                actual, bad,
                "canonical {field} bytes must survive unchanged"
            );
        }
    }
}
#[test]
fn query_source_project_gate_cannot_skip_bad_old_excluded_rows_and_clears_after_correction() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/raw-gate", "old excluded", Actor::User, GOOD);
    draft.updated_at = format!("{GOOD}\0junk");
    let mut bad = store.create_tracker_task(draft, &[]).unwrap();
    let good = store
        .create_tracker_task(
            Task::drafted("/raw-gate", "visible", Actor::User, "2026-10-02T00:00:00Z"),
            &[],
        )
        .unwrap();
    assert_eq!(
        store
            .list_task_query_candidates("/raw-gate", TaskQueryOrder::Updated, None, 1)
            .unwrap()[0]
            .0
            .row["id"],
        good.id
    );
    domain(
        &store,
        "/raw-gate",
        TaskQueryOrder::Updated,
        false,
        Some("updated_at"),
    );
    bad.updated_at = GOOD.into();
    store.save_tracker_task_activity(&bad, &[], &[]).unwrap();
    domain(&store, "/raw-gate", TaskQueryOrder::Updated, false, None);
    bad.updated_at = format!("{GOOD}\0junk");
    store.save_tracker_task_activity(&bad, &[], &[]).unwrap();
    domain(
        &store,
        "/raw-gate",
        TaskQueryOrder::Updated,
        false,
        Some("updated_at"),
    );
    store
        .delete_tracker_tasks_of_project("/raw-gate", |_| Ok(()))
        .unwrap();
    domain(&store, "/raw-gate", TaskQueryOrder::Updated, false, None);
}
#[test]
fn query_source_done_event_fallback_and_valid_session_bypass_use_selected_sources() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/raw-done-event", "done event", Actor::User, GOOD);
    draft.status = "done".into();
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    let moved = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Moved,
        serde_json::json!({"to":"done"}),
        &format!("{GOOD}\0junk"),
    );
    store
        .save_tracker_task_activity(&task, &[], &[moved])
        .unwrap();
    domain(
        &store,
        "/raw-done-event",
        TaskQueryOrder::Number,
        true,
        Some("done_at"),
    );
    task.done_at = Some("".into());
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    domain(
        &store,
        "/raw-done-event",
        TaskQueryOrder::Number,
        true,
        None,
    );
    task.done_at = Some(format!("{GOOD}\0junk"));
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    domain(
        &store,
        "/raw-done-event",
        TaskQueryOrder::Number,
        true,
        Some("done_at"),
    );
    task.status = "backlog".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    domain(
        &store,
        "/raw-done-event",
        TaskQueryOrder::Number,
        true,
        None,
    );
    let mut draft = Task::drafted("/raw-session", "safe session", Actor::User, GOOD);
    draft.watched = true;
    draft.created_at = format!("{GOOD}\0junk");
    draft.updated_at = format!("{GOOD}\0junk");
    draft.activity_session = Some(crate::session_summary::SessionSummary {
        session_started_ms: Some(1),
        last_activity_ms: Some(2),
    });
    store.create_tracker_task(draft, &[]).unwrap();
    domain(&store, "/raw-session", TaskQueryOrder::Anchor, false, None);
    domain(
        &store,
        "/raw-session",
        TaskQueryOrder::Updated,
        false,
        Some("updated_at"),
    );
}

#[test]
fn query_source_exact_agent_buckets_preserve_nul_prefix_and_suffix_through_history_and_reopen() {
    for agent in ["AgentA\0suffix", "\0AgentA"] {
        let dir = tempfile::tempdir().unwrap();
        let task_id = {
            let store = Store::new(dir.path()).unwrap();
            let mut draft = Task::drafted("/raw-agent", "exact agent", Actor::User, GOOD);
            draft.assignee = Some(crate::tracker::Assignee::Agent {
                agent_id: agent.into(),
            });
            let task = store.create_tracker_task(draft, &[]).unwrap();
            assert_agent_sources(&store, &task.id, agent);
            store
                .save_tracker_task_activity(
                    &task,
                    &[said(&task, "tc-00000000000000000000000090", GOOD)],
                    &[],
                )
                .unwrap();
            assert_agent_sources(&store, &task.id, agent);
            store
                .repair_task_summaries(std::slice::from_ref(&task.id))
                .unwrap();
            assert_agent_sources(&store, &task.id, agent);
            task.id
        };
        let store = Store::new(dir.path()).unwrap();
        assert_agent_sources(&store, &task_id, agent);
    }
}
fn assert_agent_sources(store: &Store, task_id: &str, agent: &str) {
    let counts = store
        .task_counts("/raw-agent", &[agent.into(), "AgentA".into()])
        .unwrap();
    assert_eq!(
        counts
            .iter()
            .map(|c| c.unfinished_open_count)
            .collect::<Vec<_>>(),
        [1, 1, 0],
        "agent {agent:?}"
    );
    let fact: String = store
        .connection()
        .query_row(
            "SELECT assignee_agent_id FROM tracker_task_query_facts WHERE task_id=?1",
            [task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fact, agent);
    assert_eq!(
        store.load_tracker_task(task_id).unwrap().unwrap().assignee,
        Some(crate::tracker::Assignee::Agent {
            agent_id: agent.into()
        })
    );
}

#[test]
fn query_source_status_done_prefix_is_not_completion_and_labels_remain_exact() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/raw-status", "status", Actor::User, GOOD);
    draft.status = "done\0suffix".into();
    draft.labels = vec!["api\0suffix".into()];
    let task = store.create_tracker_task(draft, &[]).unwrap();
    assert_eq!(
        store.task_counts("/raw-status", &[]).unwrap()[0].unfinished_open_count,
        1
    );
    let fact: bool = store
        .connection()
        .query_row(
            "SELECT unfinished FROM tracker_task_query_facts WHERE task_id=?1",
            [&task.id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(fact);
    let row = store
        .task_summary_by_number("/raw-status", task.number)
        .unwrap()
        .unwrap()
        .row;
    assert_eq!(row["status"], "done\0suffix");
    assert_eq!(row["labels"], serde_json::json!(["api\0suffix"]));
}

#[test]
fn query_source_read_watch_dismiss_and_fact_suffixes_preserve_nul_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/raw-marks", "markers", Actor::User, GOOD);
    draft.watched = true;
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    let mut note = said(&task, "tc-zzzz", GOOD);
    note.mentions_user = true;
    store
        .save_tracker_task_activity(&task, &[note], &[])
        .unwrap();
    task.read_through = Some("\0-zzzz".into());
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(
        store.task_counts("/raw-marks", &[]).unwrap()[0].watched_unread_total,
        0,
        "Rust when splits after embedded NUL"
    );
    task.read_through = None;
    task.watch_started_after = Some("\0-zzzz".into());
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(
        store.task_counts("/raw-marks", &[]).unwrap()[0].watched_unread_total,
        0
    );
    task.watch_started_after = None;
    task.dismissed_through = Some("\0-zzzz".into());
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert!(!store
        .task_query_matches(
            &task.id,
            &crate::store::TaskQueryFilter {
                attention_rule: Some("legacy".into()),
                ..Default::default()
            }
        )
        .unwrap());
    task.dismissed_through = None;
    task.read_through = Some("tc-a".into());
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let note = said(&task, "tc-a\0z", GOOD);
    store
        .save_tracker_task_activity(&task, &[note], &[])
        .unwrap();
    let canonical = store.load_tracker_task(&task.id).unwrap().unwrap();
    let timeline = store.load_tracker_timeline(&task.id).unwrap();
    assert_eq!(
        store.task_counts("/raw-marks", &[]).unwrap()[0].watched_unread_total,
        crate::tracker::summary::unread_since_mark(&canonical, &timeline) as u64
    );
}
#[test]
fn query_source_pending_kind_truthiness_preserves_leading_nul() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = store
        .create_tracker_task(Task::drafted("/raw-held", "held", Actor::User, GOOD), &[])
        .unwrap();
    // Explicit metadata import: DeliveryIntent.target is JSON Value, not an enum.
    store.connection().execute("INSERT INTO tracker_delivery_intents(id,task_id,project_key,generation,operation_id,state,record) VALUES('raw-held',?1,'/raw-held',1,'raw-held-op','queued',?2)",rusqlite::params![task.id,serde_json::json!({"target":{"kind":"\0agent","agent_id":"agent-1"}}).to_string()]).unwrap();
    store
        .repair_task_summaries(std::slice::from_ref(&task.id))
        .unwrap();
    assert!(store
        .task_query_matches(
            &task.id,
            &crate::store::TaskQueryFilter {
                held: Some(true),
                undelivered: Some(true),
                ..Default::default()
            }
        )
        .unwrap());
}

#[test]
fn query_source_marker_domain_uses_utf8_bytes_and_project_scope_and_clears_after_repair() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/raw-marker-domain", "old row", Actor::User, GOOD);
    draft.read_through = Some("é".repeat(64));
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    assert!(
        store.task_counts("/raw-marker-domain", &[]).is_ok(),
        "128 UTF8 bytes remain supported"
    );
    task.read_through = Some(format!("{}x", "é".repeat(64)));
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    store
        .create_tracker_task(
            Task::drafted("/raw-marker-domain", "new visible", Actor::User, GOOD),
            &[],
        )
        .unwrap();
    assert!(
        store.task_counts("/raw-marker-domain", &[]).is_err(),
        "129 UTF8 bytes cannot silently feed exact aggregates even in an older row"
    );
    task.read_through = None;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    store
        .repair_task_summaries(std::slice::from_ref(&task.id))
        .unwrap();
    assert!(store.task_counts("/raw-marker-domain", &[]).is_ok());
    task.dismissed_through = Some("x".repeat(129));
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert!(store.task_counts("/raw-marker-domain", &[]).is_err());
    store
        .delete_tracker_tasks_of_project("/raw-marker-domain", |_| Ok(()))
        .unwrap();
    assert!(store.task_counts("/raw-marker-domain", &[]).is_ok());
}

#[test]
fn query_source_marker_profile_matches_canonical_first_hyphen_without_truthy_normalization() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/raw-profile", "canonical markers", Actor::User, GOOD);
    draft.watched = true;
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    let mut watch = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Watched,
        serde_json::json!({}),
        GOOD,
    );
    watch.id = "te-zzzz".into();
    let notes = [said(&task, "tc-aa\0-bb", GOOD), said(&task, "tc-é-ß", GOOD)];
    store
        .save_tracker_task_activity(&task, &notes, &[watch])
        .unwrap();
    let cases = [
        None,
        Some(""),
        Some(" \t"),
        Some("\0-zzzz"),
        Some("pré-é-ß"),
        Some("é"),
        Some("x-\0é-x"),
        Some("pré-"),
    ];
    for field in ["read", "watch", "dismiss"] {
        for mark in cases {
            task.read_through = None;
            task.watch_started_after = Some("".into());
            task.dismissed_through = None;
            match field {
                "read" => task.read_through = mark.map(str::to_string),
                "watch" => task.watch_started_after = mark.map(str::to_string),
                _ => task.dismissed_through = mark.map(str::to_string),
            };
            store.save_tracker_task_activity(&task, &[], &[]).unwrap();
            let canonical = store.load_tracker_task(&task.id).unwrap().unwrap();
            let timeline = store.load_tracker_timeline(&task.id).unwrap();
            let unread = crate::tracker::summary::unread_since_mark(&canonical, &timeline);
            assert_eq!(
                store.task_counts("/raw-profile", &[]).unwrap()[0].watched_unread_total,
                unread as u64,
                "{field} {mark:?}"
            );
            let has_comments=timeline.iter().any(|entry|matches!(entry,crate::tracker::TimelineEntry::Comment(note) if crate::tracker::summary::after(canonical.read_through.as_deref(),&note.id)));
            let meaningful_after = timeline
                .iter()
                .filter(|entry| !crate::tracker::summary::attention_bookkeeping(entry))
                .any(|entry| {
                    crate::tracker::summary::after(
                        canonical.dismissed_through.as_deref(),
                        crate::tracker::summary::entry_id(entry),
                    )
                });
            let legacy = store
                .task_query_matches(
                    &task.id,
                    &crate::store::TaskQueryFilter {
                        attention_rule: Some("legacy".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(
                legacy,
                unread > 0 && has_comments && meaningful_after,
                "{field} {mark:?}"
            );
            let actual = match field {
                "read" => canonical.read_through,
                "watch" => canonical.watch_started_after,
                _ => canonical.dismissed_through,
            };
            assert_eq!(actual.as_deref(), mark);
        }
    }
}

#[test]
fn query_source_old_prefix_projection_rebuilds_all_layers_without_canonical_rewrites() {
    let dir = tempfile::tempdir().unwrap();
    let (id, done_id, canonical_task, canonical_note, basis) = {
        let store = Store::new(dir.path()).unwrap();
        let mut draft = Task::drafted(
            "/raw-upgrade",
            "old prefix",
            Actor::Agent {
                agent_id: "Creator\0suffix".into(),
            },
            GOOD,
        );
        draft.status = "done\0suffix".into();
        draft.watched = true;
        draft.updated_at = format!("{GOOD}\0junk");
        draft.created_at = format!("{GOOD}\0junk");
        draft.labels = vec!["api\0suffix".into()];
        draft.read_through = Some("\0-zzzz".into());
        draft.assignee = Some(crate::tracker::Assignee::Agent {
            agent_id: "AgentA\0suffix".into(),
        });
        let task = store.create_tracker_task(draft, &[]).unwrap();
        let note = said(&task, "tc-zzzz", &format!("{GOOD}\0junk"));
        store
            .save_tracker_task_activity(&task, std::slice::from_ref(&note), &[])
            .unwrap();
        let mut done = Task::drafted("/raw-upgrade", "done input", Actor::User, GOOD);
        done.status = "done".into();
        done.done_at = Some(format!("{GOOD}\0junk"));
        let done = store.create_tracker_task(done, &[]).unwrap();
        let basis = store.task_query_basis().unwrap();
        let conn = store.connection();
        let canonical_task: String = conn
            .query_row(
                "SELECT record FROM tracker_tasks WHERE id=?1",
                [&task.id],
                |r| r.get(0),
            )
            .unwrap();
        let canonical_note: String = conn
            .query_row(
                "SELECT record FROM tracker_comments WHERE id=?1",
                [&note.id],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute("UPDATE tracker_task_query_facts SET invalid_updated=0,invalid_anchor=0,invalid_done=0,invalid_markers=0,unfinished=0,assignee_agent_id='AgentA',contribution_agent='AgentA',attention_asked=1 WHERE task_id=?1",[&task.id]).unwrap();
        conn.execute("UPDATE tracker_task_summary_context SET read_ulid='',unread_count=1,unread_agent_comments=1 WHERE task_id=?1",[&task.id]).unwrap();
        conn.execute("UPDATE tracker_task_summaries SET record=json_set(record,'$.unread_count',1,'$.unread_agent_comments',1) WHERE task_id=?1",[&task.id]).unwrap();
        conn.execute("DELETE FROM tracker_task_query_counts", [])
            .unwrap();
        conn.execute("INSERT INTO tracker_task_query_counts VALUES('/raw-upgrade','',2,1),('/raw-upgrade','AgentA',1,1)",[]).unwrap();
        conn.execute("UPDATE meta SET value='old-prefix-source-definition' WHERE key='task_summary_v1_definition'",[]).unwrap();
        conn.execute("UPDATE meta SET value='old-prefix-source-definition:' || (SELECT schema_version FROM pragma_schema_version) WHERE key='task_summary_v1_ready'",[]).unwrap();
        (task.id, done.id, canonical_task, canonical_note, basis)
    };
    let store = Store::new(dir.path()).unwrap();
    assert_ne!(store.task_query_basis().unwrap(), basis);
    assert_eq!(
        store
            .task_counts("/raw-upgrade", &["AgentA\0suffix".into(), "AgentA".into()])
            .unwrap()
            .iter()
            .map(|c| (c.unfinished_open_count, c.watched_unread_total))
            .collect::<Vec<_>>(),
        [(1, 0), (1, 0), (0, 0)]
    );
    domain(
        &store,
        "/raw-upgrade",
        TaskQueryOrder::Updated,
        false,
        Some("updated_at"),
    );
    domain(
        &store,
        "/raw-upgrade",
        TaskQueryOrder::Anchor,
        false,
        Some("anchor_fallback"),
    );
    domain(
        &store,
        "/raw-upgrade",
        TaskQueryOrder::Number,
        true,
        Some("done_at"),
    );
    let task = store.load_tracker_task(&id).unwrap().unwrap();
    let row = store
        .task_summary_by_number("/raw-upgrade", task.number)
        .unwrap()
        .unwrap()
        .row;
    assert_eq!(row["status"], "done\0suffix");
    assert_eq!(row["labels"], serde_json::json!(["api\0suffix"]));
    assert_eq!(row["assignee"]["agent_id"], "AgentA\0suffix");
    assert_eq!(row["created_by"]["agent_id"], "Creator\0suffix");
    assert_eq!(row["read_through"], "\0-zzzz");
    let conn = store.connection();
    let after_task: String = conn
        .query_row("SELECT record FROM tracker_tasks WHERE id=?1", [&id], |r| {
            r.get(0)
        })
        .unwrap();
    let after_note: String = conn
        .query_row(
            "SELECT record FROM tracker_comments WHERE task_id=?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after_task, canonical_task);
    assert_eq!(after_note, canonical_note);
    let layers:(bool,i64,i64,bool)=conn.query_row("SELECT f.invalid_updated,c.unread_count,json_extract(s.record,'$.unread_count'),f.attention_asked FROM tracker_task_query_facts f JOIN tracker_task_summary_context c USING(task_id) JOIN tracker_task_summaries s USING(task_id) WHERE f.task_id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(layers, (true, 0, 0, false));
    let done_invalid: bool = conn
        .query_row(
            "SELECT invalid_done FROM tracker_task_query_facts WHERE task_id=?1",
            [&done_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(done_invalid);
}

#[test]
fn query_source_reassignment_marker_failure_rolls_back_and_empty_agent_cannot_duplicate_project() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/raw-rollback", "atomic sources", Actor::User, GOOD);
    draft.assignee = Some(crate::tracker::Assignee::Agent {
        agent_id: "AgentA\0suffix".into(),
    });
    let original = store.create_tracker_task(draft, &[]).unwrap();
    let basis = store.task_query_basis().unwrap();
    store.connection().execute_batch("CREATE TRIGGER reject_exact_source BEFORE INSERT ON tracker_task_query_facts BEGIN SELECT RAISE(ABORT,'injected source failure'); END").unwrap();
    let mut changed = original.clone();
    changed.assignee = Some(crate::tracker::Assignee::Agent {
        agent_id: "AgentA".into(),
    });
    changed.read_through = Some("x".repeat(129));
    assert!(store
        .save_tracker_task_activity(&changed, &[], &[])
        .is_err());
    assert_eq!(
        store.load_tracker_task(&original.id).unwrap().unwrap(),
        original
    );
    assert_eq!(store.task_query_basis().unwrap(), basis);
    assert!(!store
        .task_query_markers_unsupported("/raw-rollback")
        .unwrap());
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_exact_source")
        .unwrap();
    store
        .save_tracker_task_activity(&changed, &[], &[])
        .unwrap();
    assert!(store.task_counts("/raw-rollback", &[]).is_err());
    changed.read_through = None;
    store
        .save_tracker_task_activity(&changed, &[], &[])
        .unwrap();
    assert_eq!(
        store
            .task_counts("/raw-rollback", &["AgentA\0suffix".into(), "AgentA".into()])
            .unwrap()
            .iter()
            .map(|v| v.unfinished_open_count)
            .collect::<Vec<_>>(),
        [1, 0, 1]
    );
    changed.assignee = Some(crate::tracker::Assignee::Agent {
        agent_id: "".into(),
    });
    store
        .save_tracker_task_activity(&changed, &[], &[])
        .unwrap();
    assert_eq!(
        store.task_counts("/raw-rollback", &[]).unwrap()[0].unfinished_open_count,
        1,
        "invalid empty agent IDs must not alias the project bucket"
    );
    store
        .delete_tracker_tasks_of_project("/raw-rollback", |_| Ok(()))
        .unwrap();
    assert_eq!(
        store.task_counts("/raw-rollback", &[]).unwrap()[0].unfinished_open_count,
        0
    );
}
