use super::*;
use crate::store::{Store, SCHEMA_VERSION};

#[test]
fn schema20_upgrade_creates_empty_verdict_journal_without_changing_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = store
        .create_tracker_task(
            crate::tracker::Task::drafted(
                "/repo",
                "retained",
                crate::tracker::Actor::User,
                "2026-10-08T00:00:00Z",
            ),
            &[],
        )
        .unwrap();
    store.connection().execute_batch("DROP TABLE review_verdict_deliveries; DROP TABLE review_revision_dispatches; UPDATE meta SET value='20' WHERE key='schema_version'").unwrap();
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    assert_eq!(SCHEMA_VERSION, 26);
    assert_eq!(reopened.load_tracker_task(&task.id).unwrap().unwrap(), task);
    assert!(reopened
        .pending_review_verdict_deliveries(10)
        .unwrap()
        .is_empty());
    let conn = reopened.connection();
    let schema: i64 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(schema, 26);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM review_revision_dispatches",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

use crate::reviews::model::ReviewSnapshot;
use crate::store::tracker::append_activity;
use crate::tracker::{
    Actor, ReviewCommentAnchor, ReviewCommentSide, ReviewOpinion, ReviewVerdict, Task, TaskComment,
};
use serde_json::json;

const NOW: &str = "2026-10-08T00:00:00Z";

fn comment(task: &Task, id: &str, actor: Actor, opinion: bool, anchor: bool) -> TaskComment {
    TaskComment {
        review_notice: None,
        id: id.into(),
        task_id: task.id.clone(),
        author: actor,
        body: format!("Comment {id}"),
        refs: vec![],
        attachments: vec![],
        created_at: NOW.into(),
        author_context: None,
        anchor: anchor.then(|| {
            Box::new(ReviewCommentAnchor {
                snapshot_id: "s1".into(),
                directory_id: "d1".into(),
                path: "src/main.rs".into(),
                side: ReviewCommentSide::Old,
                line: 42,
                commit: Some("abc123".into()),
            })
        }),
        opinion: opinion.then(|| {
            Box::new(ReviewOpinion {
                snapshot_id: "s1".into(),
                verdict: ReviewVerdict::RequestChanges,
            })
        }),
        reply_to: None,
        mentions_user: false,
        notifies_user: false,
    }
}

fn fixture(store: &Store) -> Task {
    let task = store
        .create_tracker_task(Task::drafted("/repo", "review", Actor::User, NOW), &[])
        .unwrap();
    store
        .save_review_snapshot(
            &task.id,
            "ws1",
            0,
            ReviewSnapshot {
                publication: None,
                id: "s1".into(),
                number: 0,
                created_at: NOW.into(),
                author: Actor::User,
                directories: vec![],
            },
        )
        .unwrap();
    let mut header = super::super::load_header(&store.connection(), &task.id)
        .unwrap()
        .unwrap();
    header.mode = crate::reviews::model::ReviewMode::PullRequest;
    header.pull_request=Some(serde_json::from_value(json!({"status":"open","creator":{"kind":"user"},"originating_workspace_id":"ws1","latest_published_snapshot_id":"s1","directories":[]})).unwrap());
    store
        .in_transaction(|tx| super::super::write_header(tx, &header))
        .unwrap();
    task
}

fn accept(store: &Store, task: &Task, comments: &[TaskComment]) {
    store
        .in_transaction(|tx| {
            append_activity(tx, comments, &[])?;
            enqueue_verdict_deliveries(tx, task, comments)
        })
        .unwrap();
}

#[test]
fn journal_freezes_only_deciders_batch_since_previous_verdict_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = fixture(&store);
    let other = Actor::Agent {
        agent_id: "advisory".into(),
    };
    let first = [
        comment(&task, "c1", Actor::User, false, true),
        comment(&task, "c2", Actor::User, true, true),
    ];
    accept(&store, &task, &first);
    let second = [
        comment(&task, "c3", Actor::User, false, true),
        comment(&task, "c4", other.clone(), false, true),
        comment(&task, "c5", other, true, false),
        comment(&task, "c6", Actor::User, true, false),
    ];
    accept(&store, &task, &second);
    // Repeating the original accepted verdict never changes its batch.
    accept(&store, &task, &first[1..]);
    assert!(store.load_review_verdict_delivery("c5").unwrap().is_none());
    let first = store.load_review_verdict_delivery("c2").unwrap().unwrap();
    assert_eq!(
        first
            .batch
            .iter()
            .map(|c| c.comment_id.as_str())
            .collect::<Vec<_>>(),
        vec!["c1", "c2"]
    );
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    let pending = reopened.pending_review_verdict_deliveries(10).unwrap();
    assert_eq!(pending.len(), 2);
    let second = &pending[1];
    assert_eq!(
        second
            .batch
            .iter()
            .map(|c| c.comment_id.as_str())
            .collect::<Vec<_>>(),
        vec!["c3"]
    );
    assert_eq!(second.batch[0].anchor.commit.as_deref(), Some("abc123"));
    assert_eq!(second.batch[0].anchor.side, ReviewCommentSide::Old);
    assert_eq!(second.revision, 1);
    assert_eq!(second.task_number, task.number);
}

#[test]
fn pending_binding_and_terminal_receipts_are_idempotent_and_retry_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = fixture(&store);
    // This journal-unit fixture bypasses lifecycle status calculation. Keep
    // its current status aligned with the one verdict being retried.
    store.connection().execute("UPDATE reviews SET record=json_set(record,'$.pull_request.status','changes_requested') WHERE task_id=?1",[&task.id]).unwrap();
    accept(
        &store,
        &task,
        &[comment(&task, "verdict", Actor::User, true, false)],
    );
    store
        .bind_review_verdict_delivery("verdict", "author")
        .unwrap();
    let changes = store.connection().total_changes();
    store
        .bind_review_verdict_delivery("verdict", "author")
        .unwrap();
    assert_eq!(
        store.connection().total_changes(),
        changes,
        "an unchanged binding must not rewrite journal or summaries"
    );

    let fallback = store
        .reserve_review_verdict_fallback_comment("verdict")
        .unwrap();
    let initial = store
        .load_review_verdict_delivery("verdict")
        .unwrap()
        .unwrap();
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    assert_eq!(
        reopened
            .reserve_review_verdict_fallback_comment("verdict")
            .unwrap(),
        fallback
    );
    let restored = reopened
        .load_review_verdict_delivery("verdict")
        .unwrap()
        .unwrap();
    assert_eq!(restored.operation_id, initial.operation_id);
    assert_eq!(restored.recipient_agent_id.as_deref(), Some("author"));
    reopened
        .fail_review_verdict_delivery("verdict", "uncertain handoff")
        .unwrap();
    assert!(reopened
        .pending_review_verdict_deliveries(10)
        .unwrap()
        .is_empty());
    assert!(reopened
        .retry_review_verdict_delivery("wrong", "verdict")
        .is_err());
    reopened
        .retry_review_verdict_delivery(&task.id, "verdict")
        .unwrap();
    let retried = reopened
        .load_review_verdict_delivery("verdict")
        .unwrap()
        .unwrap();
    assert_eq!(retried.operation_id, initial.operation_id);
    assert_eq!(retried.attempts, 1);
    assert_eq!(retried.state, ReviewVerdictDeliveryState::Pending);
    reopened
        .finish_review_verdict_delivery("verdict", Some("replacement"))
        .unwrap();
    reopened
        .fail_review_verdict_delivery("verdict", "late failure")
        .unwrap();
    assert_eq!(
        reopened
            .load_review_verdict_delivery("verdict")
            .unwrap()
            .unwrap()
            .state,
        ReviewVerdictDeliveryState::Delivered
    );
    assert!(reopened
        .retry_review_verdict_delivery(&task.id, "verdict")
        .is_err());
}

#[test]
fn verdict_acceptance_and_journal_roll_back_together() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = fixture(&store);
    let verdict = comment(&task, "rollback", Actor::User, true, false);
    let result = store.in_transaction(|tx| {
        append_activity(tx, std::slice::from_ref(&verdict), &[])?;
        enqueue_verdict_deliveries(tx, &task, std::slice::from_ref(&verdict))?;
        Err::<(), _>(invalid("force transaction rollback"))
    });
    assert!(result.is_err());
    assert!(store.load_tracker_comment("rollback").unwrap().is_none());
    assert!(store
        .load_review_verdict_delivery("rollback")
        .unwrap()
        .is_none());
}

#[test]
fn journal_keeps_each_verdict_in_acceptance_order_after_binding_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = fixture(&store);
    for (id, verdict) in [
        ("v1", ReviewVerdict::RequestChanges),
        ("v2", ReviewVerdict::Approve),
        ("v3", ReviewVerdict::Reject),
    ] {
        let mut comment = comment(&task, id, Actor::User, true, false);
        comment.opinion.as_mut().unwrap().verdict = verdict;
        accept(&store, &task, &[comment]);
        assert_eq!(
            store
                .load_review_verdict_delivery(id)
                .unwrap()
                .unwrap()
                .verdict,
            verdict
        );
    }
    assert_eq!(
        store.pending_review_verdict_deliveries(1).unwrap()[0].comment_id,
        "v1"
    );
    store
        .bind_review_verdict_delivery("v1", "stopped-author")
        .unwrap();
    assert_eq!(
        store.pending_review_verdict_deliveries(1).unwrap()[0].comment_id,
        "v1"
    );
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    assert_eq!(
        reopened.pending_review_verdict_deliveries(1).unwrap()[0].comment_id,
        "v1"
    );
}

#[test]
fn redirect_retires_only_operations_proven_not_to_have_reached_the_recipient() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = fixture(&store);
    for (id, status, error, expected) in [
        ("queued", "queued", None, true),
        ("claimed", "claimed", None, false),
        ("uncertain", "uncertain", None, false),
        ("success", "delivered", None, false),
        ("failure", "delivered", Some("removed before handoff"), true),
    ] {
        accept(
            &store,
            &task,
            &[comment(&task, id, Actor::User, true, false)],
        );
        store
            .bind_review_verdict_delivery(id, "removed-author")
            .unwrap();
        let original = store.load_review_verdict_delivery(id).unwrap().unwrap();
        store.connection().execute("INSERT INTO operations(operation_id,method,entity_id,agent_id,conversation_id,choice_revision,request_hash,posted_sequence,message_start_sequence,status,execution_error,created_at,updated_at) VALUES(?1,'thread.post','owner','removed-author','thread',0,'hash',1,1,?2,?3,?4,?4)", params![original.operation_id,status,error,NOW]).unwrap();
        assert_eq!(
            store
                .redirect_review_verdict_delivery(id, &original.operation_id)
                .unwrap(),
            expected,
            "{status}/{error:?}"
        );
        let redirected = store.load_review_verdict_delivery(id).unwrap().unwrap();
        if expected {
            assert_ne!(redirected.operation_id, original.operation_id);
            assert!(redirected.recipient_agent_id.is_none());
            assert!(!store
                .redirect_review_verdict_delivery(id, &original.operation_id)
                .unwrap());
            let old = store.operation(&original.operation_id).unwrap().unwrap();
            assert_eq!(old.status, crate::operation::OperationStatus::Delivered);
            assert!(old.execution_error.is_some());
        } else {
            assert_eq!(redirected.operation_id, original.operation_id);
            assert_eq!(
                redirected.recipient_agent_id.as_deref(),
                Some("removed-author")
            );
        }
    }
}

#[test]
fn public_delivery_projection_omits_comment_bodies_and_author_lookup_reads_only_header() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = fixture(&store);
    let anchored = comment(&task, "body-note", Actor::User, false, true);
    let mut verdict = comment(&task, "body-verdict", Actor::User, true, false);
    verdict.body = "Private duplicated verdict body".into();
    accept(&store, &task, &[anchored.clone(), verdict.clone()]);
    let review = store.load_review(&task.id).unwrap().unwrap();
    let summary = serde_json::to_value(&review.verdict_deliveries[0]).unwrap();
    assert!(summary.get("reason").is_none());
    assert!(summary.get("project_path").is_none());
    assert!(summary.get("operation_id").is_none());
    assert!(summary["batch"][0].get("body").is_none());
    assert_eq!(summary["batch"][0]["comment_id"], anchored.id);
    assert_eq!(
        summary["batch"][0]["anchor"],
        serde_json::to_value(anchored.anchor).unwrap()
    );
    let durable = store
        .load_review_verdict_delivery(&verdict.id)
        .unwrap()
        .unwrap();
    assert_eq!(durable.reason, verdict.body);
    assert_eq!(durable.batch[0].body, anchored.body);

    // Polling the author must not deserialize a snapshot or the historical
    // batches. A broken unrelated delivery cannot prevent header resolution.
    store
        .connection()
        .execute(
            "UPDATE review_verdict_deliveries SET record='broken' WHERE comment_id=?1",
            [&verdict.id],
        )
        .unwrap();
    assert_eq!(store.review_verdict_author(&task.id).unwrap(), None);
    assert!(store.load_review(&task.id).is_err());
}
