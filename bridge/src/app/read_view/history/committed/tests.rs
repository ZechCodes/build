use super::*;
use std::cell::Cell;
use std::sync::Mutex;

thread_local! { static COPIED: Cell<usize> = const { Cell::new(0) }; }

pub(super) fn record_chunk_copy() {
    COPIED.with(|count| count.set(count.get() + 1));
}

struct Fixture {
    _root: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    committed: CommittedThreadItems,
    sequence: u64,
}

fn digest(thread: &Thread, sequence: u64) -> Value {
    json!({"thread_id":thread.id,
        "thread_generation_revision":thread.generation_revision,"last_sequence":sequence})
}

impl Fixture {
    fn new(unrelated: usize) -> Self {
        let root = tempfile::tempdir().unwrap();
        let context = crate::harness::HarnessContext::resolved(
            root.path().join("mcp.sock"),
            root.path().to_owned(),
        )
        .unwrap();
        let app = AppState::new_unrooted_configured(root.path().join("wt"), "main", true, context);
        let mut thread = Thread::for_agent("selected");
        thread.post_user("historical sparse receipt", None, "before");
        for _ in 0..8 {
            thread.post_user("later history", None, "before");
        }
        let sequence = thread.last_sequence();
        let mut delta = SnapshotDelta::new(app.read_view.next_version())
            .put_history("selected", SharedHistory::warm(&thread, None).unwrap())
            .put(
                "agent.list",
                "selected",
                json!({"agents":[{"thread":digest(&thread,sequence)}]}),
            )
            .put(
                "workspace.get",
                "selected",
                json!({"conversation":{"thread":digest(&thread,sequence)}}),
            );
        let mut reset = thread.clone();
        reset.generation_revision += 1;
        delta = delta
            .put("agent.list", "reset", json!([digest(&reset, 0)]))
            .put("agent.list", "current", digest(&thread, sequence + 1))
            .put("agent.list", "newer", digest(&thread, sequence + 2));
        for index in 0..unrelated {
            let other = Thread::for_agent(&format!("unrelated-{index}"));
            let row = json!({"thread":digest(&other,0),"history":["unchanged body".repeat(64)]});
            delta = delta
                .put("entity.get", format!("owner-{index}"), row.clone())
                .put("workspace.get", format!("workspace-{index}"), row);
        }
        app.read_view.publish_delta(delta);
        thread.set_legacy_delivery_status(1, 1, crate::thread::MessageDeliveryStatus::Sent);
        let committed = CommittedThreadItems {
            conversation_id: thread.agent.id.clone(),
            thread_id: thread.id.clone(),
            generation_revision: thread.generation_revision,
            sequence: thread.last_sequence(),
            items: vec![thread.items[0].clone()],
        };
        Self {
            _root: root,
            state: Arc::new(Mutex::new(app)),
            committed,
            sequence,
        }
    }

    fn apply(&self) -> (usize, Value) {
        let clock = Arc::clone(&self.state.lock().unwrap().frame_clock);
        let samples = clock.capture_phases();
        let timer = clock.frame("agent.deliver");
        let previous = COPIED.replace(0);
        timer
            .lock(&self.state)
            .apply_committed_thread_items(&self.committed);
        let copied = COPIED.replace(previous);
        drop(timer);
        (copied, samples.snapshot())
    }
}

#[test]
fn sparse_committed_receipt_copies_only_advancing_matching_snapshot_chunks() {
    let fixture = Fixture::new(200);
    let before = fixture.state.lock().unwrap().read_view.load();
    let (copied, samples) = fixture.apply();
    eprintln!("sparse receipt copied {copied} snapshot chunks");
    let after = fixture.state.lock().unwrap().read_view.load();
    assert_eq!(
        after.get("agent.list", "selected").unwrap()["agents"][0]["thread"]["last_sequence"],
        fixture.committed.sequence
    );
    assert_eq!(
        after.get("workspace.get", "selected").unwrap()["conversation"]["thread"]["last_sequence"],
        fixture.committed.sequence
    );
    for (method, owner) in [
        ("agent.list", "reset"),
        ("agent.list", "current"),
        ("agent.list", "newer"),
        ("entity.get", "owner-199"),
        ("workspace.get", "workspace-199"),
    ] {
        assert_eq!(after.get(method, owner), before.get(method, owner));
    }
    assert_eq!(
        before.get("agent.list", "selected").unwrap()["agents"][0]["thread"]["last_sequence"],
        fixture.sequence
    );
    let receipt = after
        .history("selected")
        .unwrap()
        .render(&json!({"thread_after_sequence":fixture.sequence}));
    assert_eq!(receipt["items"].as_array().unwrap().len(), 1);
    assert_eq!(receipt["items"][0]["data"]["delivery_status"], "sent");
    assert_eq!(
        samples["agent.deliver"]["entered.publication.committed.chunks"]["count"],
        1
    );
    assert_eq!(
        copied, 2,
        "only the two matching, older digest chunks need copies"
    );
    let (repeated, _) = fixture.apply();
    assert_eq!(
        repeated, 0,
        "repeating an applied receipt needs no chunk copies"
    );
}
