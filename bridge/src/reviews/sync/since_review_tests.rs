//! Commits since the user's last reviewed snapshot (#427).
use super::tests::Fixture;
use crate::git_fixture::git_in;
use crate::tracker::{Actor, ReviewOpinion, ReviewVerdict, TaskComment};

fn review_latest(fixture: &Fixture, author: Actor, at: &str) -> String {
    let review = fixture.review();
    let snapshot_id = review.snapshots.last().unwrap().id.clone();
    let task = fixture
        .store
        .load_tracker_task(fixture.task_id())
        .unwrap()
        .unwrap();
    let comment = TaskComment {
        id: crate::tracker::new_comment_id(),
        task_id: task.id.clone(),
        author,
        body: "Reviewed".into(),
        anchor: None,
        reply_to: None,
        opinion: Some(Box::new(ReviewOpinion {
            snapshot_id: snapshot_id.clone(),
            verdict: ReviewVerdict::Approve,
        })),
        mentions_user: false,
        notifies_user: false,
        refs: vec![],
        attachments: vec![],
        created_at: at.into(),
        author_context: None,
    };
    fixture
        .store
        .save_tracker_task_activity(&task, &[comment], &[])
        .unwrap();
    snapshot_id
}

#[test]
fn no_user_review_leaves_the_count_absent() {
    let f = Fixture::new();
    f.commit("first.txt");
    f.push();
    review_latest(
        &f,
        Actor::Agent {
            agent_id: "agent-1".into(),
        },
        "2026-10-09T00:00:01Z",
    );
    f.sync();
    let observation = f.observation();
    assert_eq!(observation.reviewed_snapshot_id, None);
    assert_eq!(observation.commits_since_review, None);
    assert!(!observation.rewritten_since_review);
    let wire = serde_json::to_value(&observation).unwrap();
    for field in [
        "reviewed_snapshot_id",
        "commits_since_review",
        "rewritten_since_review",
    ] {
        assert!(wire.get(field).is_none(), "{field} stays off the wire");
    }
}

#[test]
fn reviewing_the_latest_snapshot_counts_none_then_pushes_count_some() {
    let f = Fixture::new();
    f.commit("first.txt");
    f.push();
    f.sync();
    let reviewed = review_latest(&f, Actor::User, "2026-10-09T00:00:01Z");
    assert!(f.sync().persisted, "a new baseline is a new observation");
    let observation = f.observation();
    assert_eq!(
        observation.reviewed_snapshot_id.as_deref(),
        Some(reviewed.as_str())
    );
    assert_eq!(observation.commits_since_review, Some(0));
    f.commit("second.txt");
    f.commit("third.txt");
    f.push();
    f.sync();
    let observation = f.observation();
    assert_eq!(
        observation.reviewed_snapshot_id.as_deref(),
        Some(reviewed.as_str())
    );
    assert_eq!(observation.commits_since_review, Some(2));
    assert!(!observation.rewritten_since_review);
    review_latest(
        &f,
        Actor::Agent {
            agent_id: "agent-1".into(),
        },
        "2026-10-09T00:00:02Z",
    );
    f.sync();
    assert_eq!(
        f.observation().reviewed_snapshot_id.as_deref(),
        Some(reviewed.as_str()),
        "an agent's opinion is not the user's review"
    );
}

#[test]
fn rewritten_history_flags_the_rewrite_without_a_count() {
    let f = Fixture::new();
    let old = f.commit("old.txt");
    f.push();
    f.sync();
    let reviewed = review_latest(&f, Actor::User, "2026-10-09T00:00:01Z");
    git_in(f.checkout(), &["commit", "--amend", "-m", "rewritten"]);
    f.commit("after.txt");
    let binding = &f.opened.review.bindings[0];
    git_in(
        f.checkout(),
        &[
            "push",
            &format!("--force-with-lease={}:{}", binding.receiving_ref, old),
        ],
    );
    f.sync();
    let observation = f.observation();
    assert_eq!(f.review().snapshots.len(), 3);
    assert_eq!(
        observation.reviewed_snapshot_id.as_deref(),
        Some(reviewed.as_str())
    );
    assert_eq!(observation.commits_since_review, None);
    assert!(observation.rewritten_since_review);
}
