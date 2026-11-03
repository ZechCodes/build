use super::sync_scopes::TaskSyncScope;
use crate::store::{sync::SyncReadConnection, Store};
use crate::tracker::{Actor, Assignee, Task};
use serde_json::{json, Value};

const NOW: &str = "2026-01-01T00:00:00Z";
fn create(store: &Store, title: &str) -> Task {
    store
        .create_tracker_task(
            Task::drafted("/synthetic-project", title, Actor::User, NOW),
            &[],
        )
        .unwrap()
}
fn snapshot(
    read: &mut SyncReadConnection,
    scope: &TaskSyncScope,
    next: Option<&str>,
    limit: usize,
) -> Value {
    read.read(|tx| scope.snapshot(tx, next, limit, "epoch-one", crate::store::sync::now_ms()))
        .unwrap()
        .unwrap()
}
fn changes(
    read: &mut SyncReadConnection,
    scope: &TaskSyncScope,
    cursor: &str,
    next: Option<&str>,
    limit: usize,
) -> Value {
    read.read(|tx| {
        scope.changes(
            tx,
            cursor,
            next,
            limit,
            "epoch-one",
            crate::store::sync::now_ms(),
        )
    })
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn snapshot_watermark_catches_a_write_before_first_changes_and_during_paging() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut oldest = create(&store, "oldest");
    let newest = create(&store, "newest");
    let scope =
        TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let first = snapshot(&mut read, &scope, None, 1);
    assert_eq!(first["records"][0]["row"]["id"], newest.id);
    oldest.title = "edited during paging".into();
    store.save_tracker_task_activity(&oldest, &[], &[]).unwrap();
    let second = snapshot(&mut read, &scope, first["next_page"].as_str(), 1);
    assert_eq!(second["watermark"], first["watermark"]);
    assert_eq!(second["records"][0]["row"]["title"], oldest.title);
    let added = create(&store, "after snapshot");
    let answer = changes(
        &mut read,
        &scope,
        first["cursor"].as_str().unwrap(),
        None,
        100,
    );
    assert_eq!(answer["changes"].as_array().unwrap().len(), 2);
    assert_eq!(answer["changes"][0]["row"]["title"], oldest.title);
    assert_eq!(answer["changes"][1]["row"]["id"], added.id);
    let repeated = changes(
        &mut read,
        &scope,
        first["cursor"].as_str().unwrap(),
        None,
        100,
    );
    assert_eq!(answer, repeated);
}

#[tokio::test]
async fn current_membership_covers_enter_leave_reassign_unwatch_and_project_delete() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = create(&store, "member");
    let scope = TaskSyncScope::project(
        "synthetic-project",
        "/synthetic-project",
        &json!({"watched":true,"assignee":{"kind":"agent","agent_id":"Actual\0suffix"}}),
    )
    .unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let empty = snapshot(&mut read, &scope, None, 10);
    assert_eq!(empty["records"], json!([]));
    task.watched = true;
    task.assignee = Some(Assignee::Agent {
        agent_id: "Actual\0suffix".into(),
    });
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let enter = changes(
        &mut read,
        &scope,
        empty["cursor"].as_str().unwrap(),
        None,
        10,
    );
    assert_eq!(enter["changes"][0]["member"], true);
    assert_eq!(
        enter["changes"][0]["row"]["assignee"]["agent_id"],
        "Actual\0suffix"
    );
    task.assignee = Some(Assignee::User);
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let leave = changes(
        &mut read,
        &scope,
        enter["cursor"].as_str().unwrap(),
        None,
        10,
    );
    assert_eq!(leave["changes"][0]["member"], false);
    task.watched = false;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let unwatch = changes(
        &mut read,
        &scope,
        leave["cursor"].as_str().unwrap(),
        None,
        10,
    );
    assert_eq!(unwatch["changes"][0]["member"], false);
    store
        .delete_tracker_tasks_of_project("/synthetic-project", |_| Ok(()))
        .unwrap();
    let deleted = changes(
        &mut read,
        &scope,
        unwatch["cursor"].as_str().unwrap(),
        None,
        10,
    );
    assert_eq!(deleted["changes"][0]["member"], false);
    assert_eq!(deleted["changes"][0]["operation"], "delete");
    assert_eq!(deleted["changes"][0]["row"], Value::Null);
}

#[tokio::test]
async fn changes_continuation_keeps_fixed_head_and_refuses_retention_gap() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let scope =
        TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let empty = snapshot(&mut read, &scope, None, 10);
    create(&store, "one");
    create(&store, "two");
    let first = changes(
        &mut read,
        &scope,
        empty["cursor"].as_str().unwrap(),
        None,
        1,
    );
    create(&store, "three");
    let second = changes(
        &mut read,
        &scope,
        empty["cursor"].as_str().unwrap(),
        first["continuation"].as_str(),
        1,
    );
    assert_eq!(second["head"], first["head"]);
    assert!(second["continuation"].is_null());
    assert!(second["cursor"].as_str().is_some());
    store
        .writer_for_test()
        .execute(
            "UPDATE sync_stream_state SET floor=head WHERE stream_kind='project_tasks'",
            [],
        )
        .unwrap();
    let gap = changes(
        &mut read,
        &scope,
        empty["cursor"].as_str().unwrap(),
        first["continuation"].as_str(),
        1,
    );
    assert_eq!(gap["status"], "resync_required");
    assert_eq!(gap["reason"], "retention_expired");
    assert!(gap.get("cursor").is_none());
}

#[tokio::test]
async fn continuation_retention_uses_applied_position_and_accepts_floor_equality() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let scope =
        TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let base = snapshot(&mut read, &scope, None, 10);
    create(&store, "one");
    create(&store, "two");
    let first = changes(&mut read, &scope, base["cursor"].as_str().unwrap(), None, 1);
    let after = first["changes"][0]["seq"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    store
        .writer_for_test()
        .execute(
            "UPDATE sync_stream_state SET floor=?1 WHERE stream_kind='project_tasks'",
            [after],
        )
        .unwrap();

    let retained = changes(
        &mut read,
        &scope,
        base["cursor"].as_str().unwrap(),
        first["continuation"].as_str(),
        1,
    );
    assert_eq!(retained["status"], "ok", "floor == applied position");

    let initial_gap = changes(&mut read, &scope, base["cursor"].as_str().unwrap(), None, 1);
    assert_eq!(initial_gap["status"], "resync_required");
    assert_eq!(initial_gap["reason"], "retention_expired");

    store
        .writer_for_test()
        .execute(
            "UPDATE sync_stream_state SET floor=?1 WHERE stream_kind='project_tasks'",
            [after + 1],
        )
        .unwrap();
    let continuation_gap = changes(
        &mut read,
        &scope,
        base["cursor"].as_str().unwrap(),
        first["continuation"].as_str(),
        1,
    );
    assert_eq!(continuation_gap["status"], "resync_required");
    assert_eq!(continuation_gap["reason"], "retention_expired");
}

#[tokio::test]
async fn nul_pending_presence_and_exact_actor_bytes_survive_every_summary_surface() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let project =
        TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let base = snapshot(&mut read, &project, None, 10);
    for (n, target) in [
        None,
        Some(Value::Null),
        Some(json!({})),
        Some(json!({"kind":""})),
        Some(json!({"kind":"\0hidden","agent_id":"Actual\0suffix"})),
    ]
    .into_iter()
    .enumerate()
    {
        let mut draft = Task::drafted(
            "/synthetic-project",
            &format!("fixture-{n}"),
            Actor::Agent {
                agent_id: "Actual\0suffix".into(),
            },
            NOW,
        );
        draft.assignee = Some(Assignee::Agent {
            agent_id: "Actual\0suffix".into(),
        });
        let task = store.create_tracker_task(draft, &[]).unwrap();
        let opened = TaskSyncScope::task(&task.id, "synthetic-project", "/synthetic-project");
        let task_base = snapshot(&mut read, &opened, None, 10);
        let mut edited = task.clone();
        edited.title.push_str(" committed");
        store.save_tracker_task_activity(&edited, &[], &[]).unwrap();
        let present = target.is_some();
        if let Some(target) = target {
            let id = format!("synthetic-intent-{n}");
            let record = json!({"id":id,"target":target,"state":"queued","error":null}).to_string();
            store.writer_for_test().execute("INSERT INTO tracker_delivery_intents(id,task_id,project_key,generation,operation_id,state,record) VALUES(?1,?2,'/synthetic-project',1,?1,'queued',?3)",rusqlite::params![id,task.id,record]).unwrap();
            store
                .repair_task_summaries(std::slice::from_ref(&task.id))
                .unwrap();
        }
        let task_page = snapshot(&mut read, &opened, None, 10);
        let task_changed = changes(
            &mut read,
            &opened,
            task_base["cursor"].as_str().unwrap(),
            None,
            100,
        );
        let pages = snapshot(&mut read, &project, None, 10);
        let changed = changes(
            &mut read,
            &project,
            base["cursor"].as_str().unwrap(),
            None,
            100,
        );
        for surface in [&task_page, &pages, &changed, &task_changed] {
            let records = surface.get("changes").unwrap_or(&surface["records"]);
            let record = records
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["key"] == task.id)
                .unwrap();
            let row = if record["kind"] == "task_head" {
                &record["row"]["task"]
            } else {
                &record["row"]
            };
            assert_eq!(
                record["selection"]["pending_present"], present,
                "fixture {n}: {surface}"
            );
            assert_eq!(row["assignee"]["agent_id"], "Actual\0suffix");
            assert_eq!(row["created_by"]["agent_id"], "Actual\0suffix");
            if n == 4 {
                assert_eq!(row["deferred_assignment"]["target"]["kind"], "");
                assert_eq!(
                    row["deferred_assignment"]["target"]["agent_id"],
                    "Actual\0suffix"
                );
            }
        }
    }
}

#[tokio::test]
async fn task_snapshot_has_a_bounded_timeline_head_and_a_watermark() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = create(&store, "opened");
    let scope = TaskSyncScope::task(&task.id, "synthetic-project", "/synthetic-project");
    let comments:Vec<crate::tracker::TaskComment>=(0..60).map(|n|serde_json::from_value(json!({"id":format!("synthetic-note-{n:03}"),"task_id":task.id,"author":{"kind":"user"},"body":"note","created_at":NOW})).unwrap()).collect();
    store
        .save_tracker_task_activity(&task, &comments, &[])
        .unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let answer = snapshot(&mut read, &scope, None, 10);
    assert_eq!(
        answer["records"][0]["row"]["timeline"]
            .as_array()
            .unwrap()
            .len(),
        50
    );
    assert_eq!(answer["records"][0]["row"]["has_more"], true);
    assert!(answer["cursor"].as_str().is_some());
    assert_eq!(answer["records"][0]["row"]["task"]["title"], "opened");
    let head = &answer["records"][0]["row"]["timeline"];
    assert_eq!(head[0]["type"], "comment");
    assert_eq!(head[0]["id"], "synthetic-note-010");
    assert_eq!(head[49]["id"], "synthetic-note-059");
}

#[tokio::test]
async fn catchup_above_500_events_requires_resync_without_a_success_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let scope =
        TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let base = snapshot(&mut read, &scope, None, 10);
    create(&store, "first");
    let five_hundred = snapshot(&mut read, &scope, None, 1);
    for n in 0..500 {
        create(&store, &format!("synthetic-task-{n}"));
    }
    let answer = changes(
        &mut read,
        &scope,
        base["cursor"].as_str().unwrap(),
        None,
        100,
    );
    assert_eq!(answer["status"], "resync_required");
    assert_eq!(answer["reason"], "over_budget");
    assert!(answer.get("cursor").is_none());
    let mut next = None;
    let mut count = 0;
    loop {
        let answer = changes(
            &mut read,
            &scope,
            five_hundred["cursor"].as_str().unwrap(),
            next.as_deref(),
            100,
        );
        assert_eq!(answer["status"], "ok");
        count += answer["changes"].as_array().unwrap().len();
        next = answer["continuation"].as_str().map(str::to_string);
        if next.is_none() {
            break;
        }
    }
    assert_eq!(count, 500, "the exact event budget is admitted");
}

#[test]
fn normalized_sync_scope_reuses_exact_890_selection_rules() {
    let a = TaskSyncScope::project(
        "p",
        "/p",
        &json!({"labels":["API","api"],"statuses":["In review","done"]}),
    )
    .unwrap();
    let b = TaskSyncScope::project(
        "p",
        "/p",
        &json!({"labels":["api"],"statuses":["done","in_review"]}),
    )
    .unwrap();
    assert_eq!(a.scope().digest(), b.scope().digest());
    assert!(TaskSyncScope::project("p", "/p", &json!({"states":[]})).is_err());
}

#[tokio::test]
async fn current_revision_can_be_newer_than_the_fixed_event_head() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let scope =
        TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let base = snapshot(&mut read, &scope, None, 10);
    create(&store, "one");
    let mut second = create(&store, "two");
    let first = changes(&mut read, &scope, base["cursor"].as_str().unwrap(), None, 1);
    second.title = "current revision".into();
    store.save_tracker_task_activity(&second, &[], &[]).unwrap();
    let last = changes(
        &mut read,
        &scope,
        base["cursor"].as_str().unwrap(),
        first["continuation"].as_str(),
        100,
    );
    let change = &last["changes"][0];
    assert_eq!(last["head"], first["head"]);
    assert_eq!(change["row"]["title"], "current revision");
    assert!(
        change["revision"].as_str().unwrap().parse::<i64>().unwrap()
            > change["seq"].as_str().unwrap().parse::<i64>().unwrap()
    );
}

#[tokio::test]
async fn recovery_rejects_bad_cross_scope_future_and_cross_purpose_tokens() {
    use crate::store::sync::{Sequence, SyncError};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    create(&store, "one");
    create(&store, "two");
    let scope =
        TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let other = TaskSyncScope::project(
        "synthetic-project",
        "/synthetic-project",
        &json!({"watched":true}),
    )
    .unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let page = snapshot(&mut read, &scope, None, 1);
    let cursor = page["cursor"].as_str().unwrap();
    read.read(|tx| {
        assert!(matches!(
            other.changes(tx, cursor, None, 10, "epoch", crate::store::sync::now_ms()),
            Err(SyncError::ScopeMismatch)
        ));
        assert!(matches!(
            scope.changes(
                tx,
                "garbled",
                None,
                10,
                "epoch",
                crate::store::sync::now_ms()
            ),
            Err(SyncError::MalformedCursor)
        ));
        let future = tx
            .sync_cursor(scope.scope(), Sequence::new(i64::MAX).unwrap())
            .unwrap();
        assert!(matches!(
            scope.changes(tx, &future, None, 10, "epoch", crate::store::sync::now_ms()),
            Err(SyncError::FutureCursor)
        ));
        assert!(matches!(
            scope.changes(
                tx,
                cursor,
                page["next_page"].as_str(),
                10,
                "epoch",
                crate::store::sync::now_ms()
            ),
            Err(SyncError::MalformedCursor)
        ));
    })
    .unwrap();
}

#[tokio::test]
async fn byte_budget_counts_envelopes_across_changes_batches() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let scope =
        TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let base = snapshot(&mut read, &scope, None, 10);
    for n in 0..200 {
        let mut draft = Task::drafted("/synthetic-project", &"x".repeat(1024), Actor::User, NOW);
        draft.labels = (0..20)
            .map(|i| format!("{i:02}{}", "y".repeat(38)))
            .collect();
        draft.trackers = (0..32)
            .map(|i| format!("{i:03}{}", "z".repeat(125)))
            .collect();
        draft.links.workspace_ids = (0..20)
            .map(|i| format!("{n:03}{i:03}{}", "w".repeat(122)))
            .collect();
        store.create_tracker_task(draft, &[]).unwrap();
    }
    let mut next = None;
    let mut bytes = 0;
    for _ in 0..10 {
        let answer = changes(
            &mut read,
            &scope,
            base["cursor"].as_str().unwrap(),
            next.as_deref(),
            20,
        );
        if answer["status"] == "resync_required" {
            assert_eq!(answer["reason"], "over_budget");
            assert!(answer.get("cursor").is_none());
            return;
        }
        assert_eq!(answer["status"], "ok");
        bytes += serde_json::to_vec(&answer).unwrap().len();
        assert!(bytes <= 1024 * 1024);
        next = answer["continuation"].as_str().map(str::to_string);
        assert!(
            next.is_some(),
            "this fixture exceeds a complete 1 MiB recovery"
        );
        read.read(|tx| {
            let decoded = tx
                .sync_decode_continuation::<Value>(
                    scope.scope(),
                    "changes",
                    next.as_deref().unwrap(),
                )
                .unwrap();
            match decoded {
                crate::store::sync::Continuation::Current(value) => assert_eq!(
                    value["bytes"].as_str().unwrap().parse::<usize>().unwrap(),
                    bytes
                ),
                _ => panic!("local continuation"),
            }
        })
        .unwrap();
    }
    panic!("byte budget was not enforced");
}

#[tokio::test]
async fn unsupported_query_domain_refuses_snapshot_and_changes_without_advancing() {
    use crate::store::sync::SyncError;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = create(&store, "one");
    let scope = TaskSyncScope::project(
        "synthetic-project",
        "/synthetic-project",
        &json!({"order":"updated"}),
    )
    .unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let base = snapshot(&mut read, &scope, None, 10);
    task.updated_at = "synthetic-unsupported-timestamp".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    read.read(|tx| {
        assert!(matches!(
            scope.snapshot(tx, None, 10, "epoch", crate::store::sync::now_ms()),
            Err(SyncError::Unavailable(_))
        ));
        assert!(matches!(
            scope.changes(
                tx,
                base["cursor"].as_str().unwrap(),
                None,
                10,
                "epoch",
                crate::store::sync::now_ms()
            ),
            Err(SyncError::Unavailable(_))
        ));
    })
    .unwrap();
}

#[tokio::test]
async fn missing_task_projection_is_unavailable_instead_of_a_false_tombstone() {
    use crate::store::sync::SyncError;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = create(&store, "one");
    let scope = TaskSyncScope::task(&task.id, "synthetic-project", "/synthetic-project");
    let mut read = store.read_pool().acquire_sync().await;
    let base = snapshot(&mut read, &scope, None, 10);
    store
        .writer_for_test()
        .execute(
            "DELETE FROM tracker_task_summaries WHERE task_id=?1",
            [&task.id],
        )
        .unwrap();
    read.read(|tx| {
        assert!(matches!(
            scope.snapshot(tx, None, 10, "epoch", crate::store::sync::now_ms()),
            Err(SyncError::Unavailable(_))
        ))
    })
    .unwrap();
    assert!(base["cursor"].as_str().is_some());
}

#[tokio::test]
async fn foreign_continuations_validate_recovery_budget_fields_before_resetting() {
    use crate::store::sync::SyncError;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let scope =
        TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let base = snapshot(&mut read, &scope, None, 10);
    create(&store, "one");
    create(&store, "two");
    let first = changes(&mut read, &scope, base["cursor"].as_str().unwrap(), None, 1);
    let (raw, tag) = first["continuation"]
        .as_str()
        .unwrap()
        .split_once('.')
        .unwrap();
    let mut payload: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(raw).unwrap()).unwrap();
    payload["incarnation"] = json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    payload["data"]["bytes"] = json!("garbled");
    let forged = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap()),
        tag
    );
    read.read(|tx| {
        assert!(matches!(
            scope.changes(
                tx,
                base["cursor"].as_str().unwrap(),
                Some(&forged),
                1,
                "epoch",
                crate::store::sync::now_ms()
            ),
            Err(SyncError::MalformedCursor)
        ))
    })
    .unwrap();
}

#[tokio::test]
async fn continuation_scope_binding_and_canonical_tags_are_independently_required() {
    use crate::store::sync::SyncError;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let a = TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let b = TaskSyncScope::project(
        "synthetic-project",
        "/synthetic-project",
        &json!({"watched":true}),
    )
    .unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let base_a = snapshot(&mut read, &a, None, 10);
    let base_b = snapshot(&mut read, &b, None, 10);
    create(&store, "one");
    create(&store, "two");
    let run_a = changes(&mut read, &a, base_a["cursor"].as_str().unwrap(), None, 1);
    let run_b = changes(&mut read, &b, base_b["cursor"].as_str().unwrap(), None, 1);
    read.read(|tx| {
        assert!(matches!(
            a.changes(
                tx,
                base_a["cursor"].as_str().unwrap(),
                run_b["continuation"].as_str(),
                1,
                "epoch",
                crate::store::sync::now_ms()
            ),
            Err(SyncError::ScopeMismatch)
        ))
    })
    .unwrap();
    let (raw, tag) = run_a["continuation"]
        .as_str()
        .unwrap()
        .split_once('.')
        .unwrap();
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let index = |ch: u8| alphabet.iter().position(|b| *b == ch).unwrap();
    assert_eq!(URL_SAFE_NO_PAD.decode(tag).unwrap().len(), 32);
    let mut variants = Vec::new();
    for pos in [0, tag.len() - 2] {
        let mut forged = tag.as_bytes().to_vec();
        forged[pos] = alphabet[index(forged[pos]) ^ 1];
        variants.push(forged);
    }
    let last = index(*tag.as_bytes().last().unwrap());
    assert_eq!(last & 3, 0);
    for spare in 1..=3 {
        let mut forged = tag.as_bytes().to_vec();
        *forged.last_mut().unwrap() = alphabet[last | spare];
        variants.push(forged);
    }
    for tag in variants {
        let token = format!("{raw}.{}", String::from_utf8(tag).unwrap());
        read.read(|tx| {
            assert!(matches!(
                a.changes(
                    tx,
                    base_a["cursor"].as_str().unwrap(),
                    Some(&token),
                    1,
                    "epoch",
                    crate::store::sync::now_ms()
                ),
                Err(SyncError::MalformedCursor)
            ))
        })
        .unwrap();
    }
}

#[tokio::test]
async fn live_snapshot_and_recovery_resume_after_an_intact_restart_with_new_reply_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = create(&store, "one");
    create(&store, "two");
    let scope =
        TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let base = snapshot(&mut read, &scope, None, 1);
    task.title = "committed before restart".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    drop(read);
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    let mut read = reopened.read_pool().acquire_sync().await;
    let page = read
        .read(|tx| {
            scope.snapshot(
                tx,
                base["next_page"].as_str(),
                1,
                "epoch-two",
                crate::store::sync::now_ms(),
            )
        })
        .unwrap()
        .unwrap();
    assert_eq!(page["incarnation"], base["incarnation"]);
    assert_eq!(page["watermark"], base["watermark"]);
    assert_eq!(page["bridge_epoch"], "epoch-two");
    let delta = read
        .read(|tx| {
            scope.changes(
                tx,
                base["cursor"].as_str().unwrap(),
                None,
                100,
                "epoch-two",
                crate::store::sync::now_ms(),
            )
        })
        .unwrap()
        .unwrap();
    assert_eq!(delta["changes"][0]["row"]["title"], task.title);
    assert_eq!(delta["bridge_epoch"], "epoch-two");
}

#[tokio::test]
async fn restore_refuses_old_snapshot_pages_and_applied_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    create(&store, "one");
    create(&store, "two");
    let scope =
        TaskSyncScope::project("synthetic-project", "/synthetic-project", &json!({})).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let base = snapshot(&mut read, &scope, None, 1);
    drop(read);
    drop(store);
    let restored = Store::new_after_restore(dir.path()).unwrap();
    let mut read = restored.read_pool().acquire_sync().await;
    let page = snapshot(&mut read, &scope, base["next_page"].as_str(), 1);
    let delta = changes(
        &mut read,
        &scope,
        base["cursor"].as_str().unwrap(),
        None,
        100,
    );
    for answer in [page, delta] {
        assert_eq!(answer["status"], "resync_required");
        assert_eq!(answer["reason"], "incarnation_changed");
        assert!(answer.get("cursor").is_none());
    }
}

#[tokio::test]
async fn anchor_order_preserves_the_890_order_field_on_pages_and_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let scope = TaskSyncScope::project(
        "synthetic-project",
        "/synthetic-project",
        &json!({"order":"anchor"}),
    )
    .unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let base = snapshot(&mut read, &scope, None, 10);
    create(&store, "anchor");
    let page = snapshot(&mut read, &scope, None, 10);
    let delta = changes(
        &mut read,
        &scope,
        base["cursor"].as_str().unwrap(),
        None,
        10,
    );
    for row in [&page["records"][0]["row"], &delta["changes"][0]["row"]] {
        assert_eq!(row["order_anchor_ms"], 1767225600000_i64);
    }
}
