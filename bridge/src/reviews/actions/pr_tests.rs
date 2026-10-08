use super::*;
use crate::reviews::sync::reconcile::tests::Fixture;
use crate::tracker::Actor;

fn request(fixture: &Fixture, snapshot_id: &str) -> ActionRequest {
    let review = fixture.review();
    let directory = review
        .snapshots
        .iter()
        .find(|snapshot| snapshot.id == snapshot_id)
        .unwrap()
        .directories[0]
        .clone();
    ActionRequest {
        params: ReviewActParams {
            task_id: fixture.task_id().into(),
            expected_version: review.version,
            snapshot_id: snapshot_id.into(),
            sources: vec![SourceSelection {
                directory_id: directory.id.clone(),
                merge: Some(MergeSelection {
                    branch: "main".into(),
                }),
                push: None,
            }],
        },
        actor: Actor::User,
        sources: vec![ActionSource {
            directory,
            source_path: fixture.source.clone(),
            error: None,
        }],
    }
}

fn refused_without_admission(fixture: &Fixture, request: &ActionRequest) {
    let before = fixture.review();
    let source = git2::Repository::open(&fixture.source).unwrap();
    let target = source.refname_to_id("refs/heads/main").unwrap();
    let error = act(&fixture.store, request, || {}).unwrap_err();
    assert!(error.contains("PR merge lifecycle"), "{error}");
    assert_eq!(
        fixture.review(),
        before,
        "no action admission or version write"
    );
    assert_eq!(source.refname_to_id("refs/heads/main").unwrap(), target);
}

#[test]
fn legacy_actions_refuse_pr_historical_snapshot_with_current_version() {
    let fixture = Fixture::new();
    fixture.commit("historical.txt");
    fixture.push();
    fixture.sync();
    let historical = fixture.review().snapshots.last().unwrap().id.clone();
    fixture.commit("current.txt");
    fixture.push();
    fixture.sync();
    assert_ne!(fixture.review().snapshots.last().unwrap().id, historical);
    let request = request(&fixture, &historical);

    refused_without_admission(&fixture, &request);
    assert!(!fixture.source.join("historical.txt").exists());
}

#[test]
fn legacy_actions_refuse_closed_pr_with_current_version() {
    let fixture = Fixture::new();
    fixture.commit("closed.txt");
    fixture.push();
    fixture.sync();
    let review = fixture.review();
    fixture
        .store
        .complete_review(fixture.task_id(), review.version, &Actor::User, "Closed")
        .unwrap();
    let request = request(&fixture, &review.snapshots.last().unwrap().id);

    refused_without_admission(&fixture, &request);
    assert!(!fixture.source.join("closed.txt").exists());
}
