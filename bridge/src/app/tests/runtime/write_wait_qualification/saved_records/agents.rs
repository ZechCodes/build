//! Actual competing Coding sockets; this control expects missing witnesses.
use super::fixture::{SavedFixture, SavedRequest};
use crate::app::accepted_records::{AcceptedRecord, AcceptedRecords, AgentRecordWitness};
use crate::app::tests::*;
use crate::mcp::BridgeAction;
use crate::store::Store;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const TOPIC_A: &str = "qual/saved-record/topic/a";
const TOPIC_B: &str = "qual/saved-record/topic/b";
const NAME_A: &str = "qual/saved-record/name/a";
const NAME_B: &str = "qual/saved-record/name/b";
const FAILED: &str = "qual/saved-record/topic/failed-save";

fn requests() -> Vec<SavedRequest> {
    vec![
        SavedRequest::Mcp {
            original_id: TOPIC_A.into(),
            action: BridgeAction::SetTopic {
                topic: "first accepted topic".into(),
            },
        },
        SavedRequest::Mcp {
            original_id: TOPIC_B.into(),
            action: BridgeAction::SetTopic {
                topic: "second accepted topic".into(),
            },
        },
        SavedRequest::Mcp {
            original_id: NAME_A.into(),
            action: BridgeAction::SetName {
                name: "First name".into(),
            },
        },
        SavedRequest::Mcp {
            original_id: NAME_B.into(),
            action: BridgeAction::SetName {
                name: "Second name".into(),
            },
        },
    ]
}

fn persisted(store: &Store, owner: &str, agent: &str) -> Value {
    // The Store clone was captured during setup. This independent SQL oracle
    // runs after the group completes and without any AppState guard.
    let saved = store
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|row| row.id == owner)
        .unwrap()
        .agents
        .into_iter()
        .find(|row| row.id == agent)
        .unwrap();
    json!({"topic":saved.topic,"name":saved.name,"name_asked":saved.name_asked})
}

fn fields(witness: &AgentRecordWitness) -> Value {
    json!({"topic":witness.topic,"name":witness.name,"name_asked":witness.name_asked})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_coding_topic_name_witnesses_bind_only_successful_installed_records() {
    crate::git_fixture::environment::isolated_git_test!();
    let records = AcceptedRecords::new();
    let fixture = SavedFixture::new(Arc::clone(&records));
    let owner = fixture.owner.clone();
    let caller = fixture.caller.clone();
    let store = fixture.store.clone();
    let before = persisted(&store, &owner, &caller);
    let replies = fixture.compete(requests()).await;
    let installed = persisted(&store, &owner, &caller);

    // A distinct request gets the real storage fault only after the competing
    // group settles. No replacement write or optimistic witness is permitted.
    store.fail_next_write();
    let failed = fixture
        .mcp_exact(
            FAILED,
            &BridgeAction::SetTopic {
                topic: "must not commit".into(),
            },
        )
        .await;
    let after_failed = persisted(&store, &owner, &caller);
    let captured: BTreeMap<_, _> = [TOPIC_A, TOPIC_B, NAME_A, NAME_B, FAILED]
        .into_iter()
        .map(|id| (id, records.take(id)))
        .collect();
    let cleanup = fixture.finish().await;

    // Actual clients/servers and receiver work are joined before any intended
    // missing-witness RED or persistence assertion can end this control.
    assert!(cleanup.is_ok(), "saved-record fixture cleanup: {cleanup:?}");
    let replies: Vec<_> = replies.into_iter().map(Result::unwrap).collect();
    assert_eq!(replies.len(), 4);
    for proof in &replies {
        assert_eq!(
            proof.reply["ok"], true,
            "{}: {}",
            proof.original_id, proof.reply
        );
    }
    let failed = failed.unwrap();
    assert_eq!(failed.original_id, FAILED);
    assert_eq!(
        failed.reply["ok"], false,
        "the actual save must fail: {}",
        failed.reply
    );
    assert!(
        failed.reply["error"]
            .as_str()
            .is_some_and(|error| error.contains("injected store failure")),
        "expected the targeted real persistence fault: {}",
        failed.reply
    );
    assert_eq!(
        after_failed, installed,
        "failed persistence installed candidate fields"
    );
    assert!(
        captured[FAILED].is_empty(),
        "failed save emitted accepted evidence"
    );
    assert_ne!(installed, before, "the real competing writes must persist");
    assert!(matches!(
        installed["topic"].as_str(),
        Some("first accepted topic" | "second accepted topic")
    ));
    assert!(matches!(
        installed["name"].as_str(),
        Some("First name" | "Second name")
    ));
    assert_eq!(installed["name_asked"], true);

    let mut snapshots = Vec::new();
    for proof in replies {
        let witnessed = &captured[proof.original_id.as_str()];
        assert_eq!(
            witnessed.len(),
            1,
            "{} must identify exactly its accepted installed record",
            proof.original_id
        );
        let AcceptedRecord::Agent(witness) = &witnessed[0] else {
            panic!("agent write emitted a different record family");
        };
        assert_eq!(witness.original_ids, vec![proof.original_id.clone()]);
        assert_eq!(witness.owner, owner);
        assert_eq!(witness.agent, caller);
        assert_eq!(
            witness.native_revision, None,
            "topic/name have no native revision"
        );
        if proof.method.ends_with("set_topic") {
            assert_eq!(json!(witness.topic), proof.reply["result"]["topic"]);
        } else {
            assert_eq!(json!(witness.name), proof.reply["result"]["name"]);
            assert!(witness.name_asked);
        }
        snapshots.push(fields(witness));
    }
    assert!(
        snapshots.contains(&installed),
        "last durable record must match an actual accepted snapshot"
    );
}
