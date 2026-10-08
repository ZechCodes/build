use super::*;
use crate::reviews::publication::registered_received_head;
use std::path::PathBuf;

fn tracking(fixture: &Fixture) -> (git2::Repository, String) {
    let binding = &fixture.review().bindings[0];
    let branch = binding
        .dedicated_branch_ref
        .strip_prefix("refs/heads/")
        .unwrap();
    (
        git2::Repository::open(fixture.checkout()).unwrap(),
        format!("refs/remotes/{}/{branch}", binding.remote_name),
    )
}

fn partial_publication(fixture: &Fixture) -> (String, String) {
    let previous = fixture.review().bindings[0].initial_head.clone();
    let head = fixture.commit("partial.txt");
    let (repository, reference) = tracking(fixture);
    let lock = repository.commondir().join(format!("{reference}.lock"));
    std::fs::write(&lock, b"foreign tracking lock").unwrap();
    let result = push(&fixture.store, &push_request(fixture, head.clone())).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Published);
    assert!(result.sources[0].recovery.is_some());
    assert_eq!(std::fs::read(&lock).unwrap(), b"foreign tracking lock");
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        previous
    );
    assert_eq!(
        registered_received_head(&fixture.review().bindings[0]).unwrap(),
        Some(head.clone())
    );
    std::fs::remove_file(lock).unwrap();
    (previous, head)
}

fn assert_repaired(fixture: &Fixture, head: &str) {
    let before = fixture.review();
    let result = push(&fixture.store, &push_request(fixture, head.into())).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Unchanged);
    assert_eq!(result.sources[0].recovery, None);
    let (repository, reference) = tracking(fixture);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        head
    );
    assert_eq!(fixture.review().version, before.version);
    assert_eq!(fixture.review().snapshots, before.snapshots);
}

#[test]
fn partial_tracking_publication_recovers_after_store_reopens() {
    let fixture = Fixture::new();
    let (_, head) = partial_publication(&fixture);
    let before = fixture.review();
    let reopened = Store::new(fixture._home.path().join("db")).unwrap();
    let result = push(&reopened, &push_request(&fixture, head.clone())).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Unchanged);
    assert_eq!(result.sources[0].recovery, None);
    assert_eq!(
        reopened.load_review(fixture.task_id()).unwrap().unwrap(),
        before
    );
    let (repository, reference) = tracking(&fixture);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        head
    );
}

#[test]
fn partial_tracking_recovery_preserves_foreign_tip_and_retains_proof() {
    let fixture = Fixture::new();
    let (previous, head) = partial_publication(&fixture);
    git_in(
        &fixture.source,
        &["commit", "--allow-empty", "-m", "foreign"],
    );
    let source = git2::Repository::open(&fixture.source).unwrap();
    let foreign = source.head().unwrap().target().unwrap();
    let (repository, reference) = tracking(&fixture);
    repository
        .reference(&reference, foreign, true, "foreign tracking writer")
        .unwrap();
    let result = push(&fixture.store, &push_request(&fixture, head.clone())).unwrap();
    assert!(result.sources[0].recovery.is_some());
    assert_eq!(repository.refname_to_id(&reference).unwrap(), foreign);
    assert_eq!(
        registered_received_head(&fixture.review().bindings[0]).unwrap(),
        Some(head.clone())
    );
    repository
        .reference(
            &reference,
            git2::Oid::from_str(&previous).unwrap(),
            true,
            "restore owned expectation",
        )
        .unwrap();
    assert_repaired(&fixture, &head);
}

fn claim_path(repository: &git2::Repository) -> PathBuf {
    std::fs::read_dir(repository.commondir().join("build-review-tracking"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}

#[test]
fn partial_tracking_recovery_preserves_changed_claim_and_retains_proof() {
    let fixture = Fixture::new();
    let (previous, head) = partial_publication(&fixture);
    let (repository, reference) = tracking(&fixture);
    let path = claim_path(&repository);
    let original = std::fs::read(&path).unwrap();
    let mut claim: serde_json::Value = serde_json::from_slice(&original).unwrap();
    claim["source_id"] = serde_json::json!("foreign-source");
    let foreign = serde_json::to_vec(&claim).unwrap();
    std::fs::write(&path, &foreign).unwrap();
    let result = push(&fixture.store, &push_request(&fixture, head.clone())).unwrap();
    assert!(result.sources[0].recovery.is_some());
    assert_eq!(std::fs::read(&path).unwrap(), foreign);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        previous
    );
    assert_eq!(
        registered_received_head(&fixture.review().bindings[0]).unwrap(),
        Some(head.clone())
    );
    std::fs::write(path, original).unwrap();
    assert_repaired(&fixture, &head);
}

#[test]
fn stale_receiver_cannot_use_tracking_recovery_to_rewind_publication() {
    let fixture = Fixture::new();
    let (previous, head) = partial_publication(&fixture);
    let mut request = push_request(&fixture, head.clone());
    request.sources[0].force_with_lease = true;
    git_in(
        &fixture.source,
        &["commit", "--allow-empty", "-m", "new receiver"],
    );
    let source = git2::Repository::open(&fixture.source).unwrap();
    let advanced = source.head().unwrap().target().unwrap();
    let binding = fixture.review().bindings.remove(0);
    crate::reviews::publication::import_publication_commit(
        &binding.receiving_repository,
        &fixture.source,
        &advanced.to_string(),
    )
    .unwrap();
    let receiver = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    receiver
        .reference(
            &binding.receiving_ref,
            advanced,
            true,
            "new receiver writer",
        )
        .unwrap();
    let result = push(&fixture.store, &request).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Failed);
    assert_eq!(
        receiver.refname_to_id(&binding.receiving_ref).unwrap(),
        advanced
    );
    let (repository, reference) = tracking(&fixture);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        previous
    );
}

#[test]
fn stale_working_head_and_review_version_cannot_repair_tracking() {
    let fixture = Fixture::new();
    let (previous, head) = partial_publication(&fixture);
    let mut request = push_request(&fixture, head.clone());
    request.expected_version -= 1;
    assert!(push(&fixture.store, &request)
        .unwrap_err()
        .starts_with("stale_version:"));
    request.expected_version += 1;
    fixture.commit("advanced.txt");
    let result = push(&fixture.store, &request).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Failed);
    let (repository, reference) = tracking(&fixture);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        previous
    );
    assert_eq!(
        registered_received_head(&fixture.review().bindings[0]).unwrap(),
        Some(head)
    );
}

fn retained_settled_proof(fixture: &Fixture) -> String {
    let (_, head) = partial_publication(fixture);
    let (repository, reference) = tracking(fixture);
    repository
        .reference(
            &reference,
            git2::Oid::from_str(&head).unwrap(),
            true,
            "settled before cleanup crash",
        )
        .unwrap();
    head
}

#[test]
fn settled_proof_retained_after_cleanup_failure_can_extend_publication() {
    let fixture = Fixture::new();
    retained_settled_proof(&fixture);
    let head = fixture.commit("next-publication.txt");
    let result = push(&fixture.store, &push_request(&fixture, head.clone())).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Published);
    assert_eq!(result.sources[0].recovery, None);
    let (repository, reference) = tracking(&fixture);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        head
    );
}

#[test]
fn multiple_extensions_preserve_the_last_authorized_tracking_tip() {
    let fixture = Fixture::new();
    let settled = retained_settled_proof(&fixture);
    let head = fixture.commit("next-publication.txt");
    let (repository, reference) = tracking(&fixture);
    let lock = repository.commondir().join(format!("{reference}.lock"));
    let checks = std::cell::Cell::new(0);
    let result = push_checked(
        &fixture.store,
        &push_request(&fixture, head.clone()),
        &|| {
            checks.set(checks.get() + 1);
            if checks.get() == 4 {
                std::fs::write(&lock, b"foreign lock after old proof settlement").unwrap();
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Published);
    assert!(result.sources[0].recovery.is_some());
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        settled
    );
    assert_eq!(
        std::fs::read(&lock).unwrap(),
        b"foreign lock after old proof settlement"
    );
    std::fs::remove_file(lock).unwrap();
    let next = fixture.commit("third-publication.txt");
    let result = push(&fixture.store, &push_request(&fixture, next.clone())).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Published);
    assert_eq!(result.sources[0].recovery, None);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        next
    );
}

#[test]
fn pending_tracking_proof_keeps_its_cas_separate_from_newer_native_receiver() {
    let fixture = Fixture::new();
    retained_settled_proof(&fixture);
    let native_head = fixture.commit("native.txt");
    let binding = fixture.review().bindings.remove(0);
    crate::reviews::publication::import_publication_commit(
        &binding.receiving_repository,
        fixture.checkout(),
        &native_head,
    )
    .unwrap();
    let receiver = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    receiver
        .reference(
            &binding.receiving_ref,
            git2::Oid::from_str(&native_head).unwrap(),
            true,
            "native receiver advancement",
        )
        .unwrap();
    fixture.sync();
    let head = fixture.commit("after-native.txt");
    let result = push(&fixture.store, &push_request(&fixture, head.clone())).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Published);
    assert_eq!(result.sources[0].recovery, None);
    let (repository, reference) = tracking(&fixture);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        head
    );
    assert_eq!(
        receiver
            .refname_to_id(&binding.receiving_ref)
            .unwrap()
            .to_string(),
        head
    );
}

#[test]
fn final_check_head_switch_cannot_publish_or_repair_tracking() {
    let fixture = Fixture::new();
    let head = fixture.commit("requested.txt");
    let before = fixture.review();
    let checks = std::cell::Cell::new(0);
    let result = push_checked(&fixture.store, &push_request(&fixture, head), &|| {
        checks.set(checks.get() + 1);
        if checks.get() == 3 {
            git_in(
                fixture.checkout(),
                &["symbolic-ref", "HEAD", "refs/heads/main"],
            );
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Failed);
    assert_eq!(
        registered_received_head(&before.bindings[0]).unwrap(),
        before.bindings[0].last_received_head
    );
    let (repository, reference) = tracking(&fixture);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        before.bindings[0].initial_head
    );
}

fn unreceived_proof(fixture: &Fixture) -> String {
    let target = fixture.commit("unreceived.txt");
    let review = fixture.review();
    let binding = &review.bindings[0];
    let (repository, _) = tracking(fixture);
    let fingerprint =
        crate::reviews::publication::tracking::fingerprint(&repository, binding).unwrap();
    fixture
        .store
        .prepare_review_tracking_expectation(
            fixture.task_id(),
            review.version,
            &binding.directory_id,
            &fingerprint,
            binding.last_received_head.as_deref(),
            &target,
        )
        .unwrap();
    target
}

#[test]
fn unreceived_proof_retarget_and_final_refusal_preserve_receiver_and_tracking() {
    let fixture = Fixture::new();
    unreceived_proof(&fixture);
    let head = fixture.commit("next.txt");
    let before = fixture.review();
    let checks = std::cell::Cell::new(0);
    let result = push_checked(
        &fixture.store,
        &push_request(&fixture, head.clone()),
        &|| {
            checks.set(checks.get() + 1);
            if checks.get() == 4 {
                git_in(
                    fixture.checkout(),
                    &["symbolic-ref", "HEAD", "refs/heads/main"],
                );
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Failed);
    assert_eq!(
        registered_received_head(&before.bindings[0]).unwrap(),
        before.bindings[0].last_received_head
    );
    let (repository, reference) = tracking(&fixture);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        before.bindings[0].initial_head
    );
    git_in(
        fixture.checkout(),
        &[
            "symbolic-ref",
            "HEAD",
            &before.bindings[0].dedicated_branch_ref,
        ],
    );
    let result = push(&fixture.store, &push_request(&fixture, head.clone())).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Published);
    assert_eq!(result.sources[0].recovery, None);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        head
    );
}

#[test]
fn newer_matching_native_receiver_and_tracking_clear_obsolete_pending_proof() {
    let fixture = Fixture::new();
    unreceived_proof(&fixture);
    let head = fixture.commit("native-next.txt");
    fixture.push();
    fixture.sync();
    let before = fixture.review();
    let (repository, reference) = tracking(&fixture);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        head
    );
    let result = push(&fixture.store, &push_request(&fixture, head.clone())).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Unchanged);
    assert_eq!(result.sources[0].recovery, None);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        head
    );
    assert_eq!(
        registered_received_head(&before.bindings[0]).unwrap(),
        Some(head)
    );
    assert_eq!(fixture.review(), before);
}

#[test]
fn newer_matching_native_receiver_and_tracking_allow_the_next_working_head() {
    let fixture = Fixture::new();
    unreceived_proof(&fixture);
    fixture.commit("native-next.txt");
    fixture.push();
    fixture.sync();
    let before = fixture.review();
    let head = fixture.commit("after-native-next.txt");
    let result = push(&fixture.store, &push_request(&fixture, head.clone())).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Published);
    assert_eq!(result.sources[0].recovery, None);
    let (repository, reference) = tracking(&fixture);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        head
    );
    assert_eq!(
        registered_received_head(&before.bindings[0]).unwrap(),
        Some(head)
    );
    assert_eq!(fixture.review().snapshots.len(), before.snapshots.len() + 1);
}

#[test]
fn same_pending_goal_uses_a_newer_matching_native_receiver_tracking_lease() {
    let fixture = Fixture::new();
    let intermediate = fixture.commit("intermediate.txt");
    let target = unreceived_proof(&fixture);
    git_in(fixture.checkout(), &["reset", "--hard", &intermediate]);
    fixture.push();
    fixture.sync();
    git_in(fixture.checkout(), &["reset", "--hard", &target]);
    let result = push(&fixture.store, &push_request(&fixture, target.clone())).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Published);
    assert_eq!(result.sources[0].recovery, None);
    let (repository, reference) = tracking(&fixture);
    assert_eq!(
        repository.refname_to_id(&reference).unwrap().to_string(),
        target
    );
    assert_eq!(
        registered_received_head(&fixture.review().bindings[0]).unwrap(),
        Some(target)
    );
}
