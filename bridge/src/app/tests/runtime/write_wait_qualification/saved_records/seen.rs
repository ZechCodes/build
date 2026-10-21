//! OFFSOURCE TESTFIRST child of saved_records. Existing native fixture, real
//! encrypted RPC/Coding sockets. Only new Seen type/enum scaffold is required
//! to compile; production emitters remain absent for the intended RED.
use super::effects_test_helpers::finish_caught;
use super::fixture::SavedFixture;
use crate::app::accepted_records::{AcceptedRecord, AcceptedRecords, SeenRecordWitness};
use crate::app::read_context::IssuedCodingAddress;
use crate::app::tests::*;
use crate::mcp::BridgeAction;
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;

fn selector(fixture: &SavedFixture) -> Value {
    let address = native_address(fixture);
    json!({"entity_id":fixture.owner,"agent_id":fixture.caller,
        "conversation_id":address.conversation,"thread_id":address.thread_id})
}
fn native_address(fixture: &SavedFixture) -> IssuedCodingAddress {
    fixture
        .state
        .lock()
        .unwrap()
        .live_mcp_issued_address(&fixture.owner, &fixture.caller)
        .unwrap()
}
fn assert_address(record: &SeenRecordWitness, original: &str, address: &IssuedCodingAddress) {
    assert_eq!(record.original_ids, vec![original]);
    assert_eq!(record.owner, address.owner);
    assert_eq!(record.agent, address.agent);
    assert_eq!(record.conversation_owner, address.canonical_owner);
    assert_eq!(record.conversation_id, address.conversation);
    assert_eq!(record.thread_id, address.thread_id);
    assert_eq!(record.generation_revision, address.generation);
    assert_eq!(record.native_revision, None);
}
fn native_head(fixture: &SavedFixture) -> u64 {
    let app = fixture.state.lock().unwrap();
    app.agent_conversation(&fixture.owner, Some(&fixture.caller))
        .unwrap()
        .last_sequence()
}
fn native_cursor(fixture: &SavedFixture) -> (u64, u64) {
    // Never SQL under AppState. Native save and published root are independent.
    let saved = fixture.store.load_attention();
    let persisted = *saved
        .get(&fixture.owner)
        .unwrap()
        .agent_read_sequences
        .get(&fixture.caller)
        .unwrap_or(&0);
    let app = fixture.state.lock().unwrap();
    let view = app.read_view.load();
    let row = view.get("agent.list", &fixture.owner).unwrap();
    let published = row["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == fixture.caller)
        .unwrap()["read_through_sequence"]
        .as_u64()
        .unwrap();
    (persisted, published)
}
async fn post(fixture: &SavedFixture, id: &str) -> Result<super::super::rpc::ReplyProof, String> {
    fixture
        .mcp_exact(
            id,
            &BridgeAction::PostThreadMessage {
                still_working: true,
                body: format!("immutable Seen control {id}"),
                options: Vec::new(),
                attachments: Vec::new(),
            },
        )
        .await
}
fn only_seen(records: Vec<AcceptedRecord>) -> Vec<SeenRecordWitness> {
    // These Seen-only control originals must not have a different record kind.
    // Normal consumer dispatch retains/archives all sibling kinds separately.
    records
        .into_iter()
        .map(|record| match record {
            AcceptedRecord::Seen(seen) => seen,
            _ => panic!("unexpected sibling kind in Seen-only control; retain diagnostic"),
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seen_retains_actual_first_target_after_later_post_and_seen() {
    crate::git_fixture::environment::isolated_git_test!();
    let records = AcceptedRecords::new();
    let fixture = SavedFixture::new(records.clone());
    let exercise = AssertUnwindSafe(async {
        let address = native_address(&fixture);
        let params = selector(&fixture);
        let first_post = post(&fixture, "qual/seen/post/first").await;
        let first_target = native_head(&fixture);
        let first = fixture
            .rpc_exact(0, "qual/seen/first", "entity.seen", params.clone())
            .await;
        let first_native = native_cursor(&fixture);
        let later_post = post(&fixture, "qual/seen/post/later").await;
        let later_target = native_head(&fixture);
        let later = fixture
            .rpc_exact(1, "qual/seen/later", "entity.seen", params)
            .await;
        let later_native = native_cursor(&fixture);
        // Retain first until after later execution, proving original immutability.
        let first_records = records.take("qual/seen/first");
        let later_records = records.take("qual/seen/later");
        (
            address,
            first_post,
            first,
            later_post,
            later,
            first_target,
            later_target,
            first_native,
            later_native,
            first_records,
            later_records,
        )
    })
    .catch_unwind()
    .await;
    let (
        address,
        first_post,
        first,
        later_post,
        later,
        first_target,
        later_target,
        first_native,
        later_native,
        first_records,
        later_records,
    ) = finish_caught(fixture, exercise).await;
    for reply in [first_post, first, later_post, later] {
        assert_eq!(reply.unwrap().reply["ok"], true);
    }
    assert!(first_target > 0 && later_target > first_target);
    assert_eq!(first_native, (first_target, first_target));
    assert_eq!(later_native, (later_target, later_target));
    // Intended missing-producer RED only AFTER all actual native semantics.
    let first = only_seen(first_records);
    let later = only_seen(later_records);
    assert_eq!(first.len(), 1, "missing exact accepted Seen original");
    assert_eq!(later.len(), 1);
    assert_eq!(first[0].actual_reached_sequence, first_target);
    assert_eq!(first[0].saved_through_sequence, first_target);
    assert_eq!(first[0].published_through_sequence, first_target);
    assert_eq!(later[0].actual_reached_sequence, later_target);
    assert_address(&first[0], "qual/seen/first", &address);
    assert_address(&later[0], "qual/seen/later", &address);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seen_noop_keeps_actual_lower_target_and_native_high_cursor() {
    crate::git_fixture::environment::isolated_git_test!();
    let records = AcceptedRecords::new();
    let fixture = SavedFixture::new(records.clone());
    let exercise = AssertUnwindSafe(async {
        let address = native_address(&fixture);
        let params = selector(&fixture);
        let posted = post(&fixture, "qual/seen/noop/post").await;
        let head = native_head(&fixture);
        let high = fixture
            .rpc_exact(0, "qual/seen/noop/high", "entity.seen", params.clone())
            .await;
        let mut lower = params;
        lower["read_through_sequence"] = json!(head.saturating_sub(1));
        let noop = fixture
            .rpc_exact(1, "qual/seen/noop/lower", "entity.seen", lower)
            .await;
        let native = native_cursor(&fixture);
        let captured = records.take("qual/seen/noop/lower");
        (address, posted, high, noop, head, native, captured)
    })
    .catch_unwind()
    .await;
    let (address, posted, high, noop, head, native, captured) =
        finish_caught(fixture, exercise).await;
    for reply in [posted, high, noop] {
        assert_eq!(reply.unwrap().reply["ok"], true);
    }
    assert!(head > 0);
    assert_eq!(native, (head, head));
    let captured = only_seen(captured);
    assert_eq!(
        captured.len(),
        1,
        "no-op target must retain native accepted outcome"
    );
    assert_eq!(captured[0].actual_reached_sequence, head - 1);
    assert_eq!(captured[0].saved_through_sequence, head);
    assert_eq!(captured[0].published_through_sequence, head);
    assert_address(&captured[0], "qual/seen/noop/lower", &address);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_seen_save_never_records_accepted_success_despite_ok_reply() {
    crate::git_fixture::environment::isolated_git_test!();
    let records = AcceptedRecords::new();
    let fixture = SavedFixture::new(records.clone());
    let exercise = AssertUnwindSafe(async {
        let params = selector(&fixture);
        let posted = post(&fixture, "qual/seen/fault/post").await;
        let before = fixture.store.load_attention().get(&fixture.owner).cloned();
        fixture.store.fail_next_write();
        let failed = fixture
            .rpc_exact(0, "qual/seen/fault", "entity.seen", params)
            .await;
        let after = fixture.store.load_attention().get(&fixture.owner).cloned();
        let captured = records.take("qual/seen/fault");
        (posted, failed, before, after, captured)
    })
    .catch_unwind()
    .await;
    let (posted, failed, before, after, captured) = finish_caught(fixture, exercise).await;
    assert_eq!(posted.unwrap().reply["ok"], true);
    assert_eq!(
        failed.unwrap().reply["ok"],
        true,
        "preserve native best-effort attention behavior"
    );
    assert_eq!(
        after, before,
        "actual targeted Store write must have failed"
    );
    assert!(
        only_seen(captured).is_empty(),
        "failed save cannot become accepted Seen evidence"
    );
}
