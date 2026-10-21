//! OFFSOURCE child of global_publication/incremental_rows; source visits only.
use super::*;
use crate::app::read_context::mcp_metadata_metrics;

#[test]
fn mcp_saved_address_work_stays_local_for_32_and_200_owners() {
    for count in [32, 200] {
        let (_tmp, mut app, _) = ownership_fixture(count, false);
        add_tip_agents(&mut app, 1);
        let (_, initial) = mcp_metadata_metrics::measure(|| app.initialize_read_context());
        assert_eq!(initial.0, count, "positive actual roster-source visits");
        assert_eq!(initial.1, count, "positive actual source indexing visits");
        let old = app.read_service().mcp_addresses();
        let agent = app.runs["run-00"].agents.iter().next().unwrap().id.clone();
        app.runs
            .get_mut("run-00")
            .unwrap()
            .agents
            .by_id_mut(&agent)
            .unwrap()
            .name = Some("one saved owner".into());
        let (_, work) = mcp_metadata_metrics::measure(|| app.publish_read_owner("run-00", true));
        assert_eq!(
            app.read_service().mcp_addresses().display_name(&agent),
            Some("one saved owner")
        );
        assert_ne!(old.display_name(&agent), Some("one saved owner"));
        assert_eq!(
            work.0, 1,
            "actual source capture stays owner-local at {count}"
        );
        assert_eq!(
            work.1, 0,
            "unique IDs need no repeated global index traversal at {count}"
        );
        let (_, metadata) = mcp_metadata_metrics::measure(|| app.publish_read_metadata_only());
        assert_eq!(
            metadata,
            (0, 0),
            "generic metadata cannot recapture roster fields"
        );
    }
}

#[test]
fn mcp_colliding_ids_keep_native_first_owner_after_take_and_reinsert() {
    let (_tmp, mut app, _) = ownership_fixture(16, false);
    add_tip_agents(&mut app, 1);
    let duplicate = app.runs["run-00"].agents.iter().next().unwrap().id.clone();
    for (owner, active) in &mut app.runs {
        let agent = active.agents.iter_mut().next().unwrap();
        agent.id = duplicate.clone();
        agent.name = Some(owner.clone());
        agent.bind_conversation(&duplicate);
    }
    app.initialize_read_context();
    for capacity in [32, 128, 512, 2048] {
        // Actual native HashMap traversal is the oracle, including any rank
        // change caused by reinsertion/rehash. A collision must refresh even
        // if this particular native map retains the same first winner.
        let owner = "run-00";
        let active = app.runs.remove(owner).unwrap();
        app.runs.reserve(capacity);
        app.runs.insert(owner.into(), active);
        let expected = app.entity_of_agent(&duplicate).unwrap();
        let (_, work) = mcp_metadata_metrics::measure(|| app.publish_read_owner(owner, true));
        let root = app.read_service().mcp_addresses();
        assert_eq!(root.issued_address(&duplicate).unwrap().owner, expected);
        assert_eq!(root.display_name(&duplicate), Some(expected.as_str()));
        assert!(
            work.1 >= 16,
            "actual native collision index must refresh on same-ID save"
        );
    }
}
