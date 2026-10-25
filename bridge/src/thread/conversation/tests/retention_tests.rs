use super::*;

#[test]
fn retention_discards_creation_prefix_without_rewinding_mutation_cursor() {
    let mut thread = Thread::for_agent("retention");
    thread.post_agent_offering("old", Vec::new(), "2026-10-01T00:00:00Z", false);
    thread.post_agent_offering("keep", Vec::new(), "2026-10-24T00:00:00Z", false);
    let high = thread.last_sequence();
    thread.apply_retention_boundary(2, high + 10, 1);
    assert_eq!(thread.items.len(), 1);
    assert_eq!(thread.items[0].sequence(), 2);
    assert_eq!(thread.history_start_sequence, Some(2));
    assert_eq!(thread.last_sequence(), high + 10);
    thread.apply_retention_boundary(1, 0, 0);
    assert_eq!(thread.history_start_sequence, Some(2));
    thread.post_agent_offering("new", Vec::new(), "2026-10-24T00:00:01Z", false);
    assert!(thread.last_sequence() > high + 10);
}
