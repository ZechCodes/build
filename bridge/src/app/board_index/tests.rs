use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::json;

use super::{
    AttentionIndex, CacheEffect, CachePublication, DiffCache, DiffCacheEntry, DiffCacheKey,
};

#[test]
fn superseded_claim_stays_held_until_that_compute_settles() {
    let mut cache = DiffCache::default();
    let key = DiffCacheKey::RunStat("run-1".to_string());
    let claim = cache.claim_refresh(key.clone()).unwrap();

    cache.supersede(&key);
    assert!(cache.claim_refresh(key.clone()).is_none());
    assert!(cache
        .publish_refresh(
            claim,
            Some(DiffCacheEntry::RunStat {
                run_id: "run-1".to_string(),
                stat: json!({"files_changed": 1}),
            }),
            Instant::now(),
        )
        .is_empty());
    assert!(cache.run_stat("run-1").is_none());
    assert!(cache.claim_refresh(key).is_some());
}

#[test]
fn release_clears_both_claim_sets_after_supersession() {
    let mut cache = DiffCache::default();
    let key = DiffCacheKey::RunStat("run-1".to_string());
    let claim = cache.claim_refresh(key.clone()).unwrap();
    cache.supersede(&key);

    cache.release_refresh(claim);

    assert!(!cache.refresh_is_running(&key));
    assert!(cache.claim_refresh(key).is_some());
}

#[test]
fn scan_failure_keeps_readable_data_and_notifies_after_each_success() {
    let mut cache = DiffCache::default();
    cache.register_project("project-1".to_string());
    let first = Instant::now();

    let success = cache
        .claim_refresh(DiffCacheKey::ExternalScan("project-1".to_string()))
        .unwrap();
    assert_eq!(
        cache.publish_refresh(
            success,
            Some(DiffCacheEntry::ExternalScan {
                project_id: "project-1".to_string(),
                worktrees: Vec::new(),
            }),
            first,
        ),
        vec![CacheEffect::BoardChanged]
    );

    let failed_once = cache
        .claim_refresh(DiffCacheKey::ExternalScan("project-1".to_string()))
        .unwrap();
    let failed_at = first + Duration::from_secs(1);
    assert_eq!(
        cache.publish_refresh(
            failed_once,
            Some(DiffCacheEntry::ExternalScanUnreadable {
                project_id: "project-1".to_string(),
            }),
            failed_at,
        ),
        vec![CacheEffect::BoardChanged]
    );
    assert!(cache.external_scan("project-1").worktrees.is_empty());
    assert_eq!(cache.external_scan("project-1").settled_at, Some(failed_at));

    let failed_twice = cache
        .claim_refresh(DiffCacheKey::ExternalScan("project-1".to_string()))
        .unwrap();
    assert!(cache
        .publish_refresh(
            failed_twice,
            Some(DiffCacheEntry::ExternalScanUnreadable {
                project_id: "project-1".to_string(),
            }),
            failed_at + Duration::from_secs(1),
        )
        .is_empty());

    let success_again = cache
        .claim_refresh(DiffCacheKey::ExternalScan("project-1".to_string()))
        .unwrap();
    assert!(cache
        .publish_refresh(
            success_again,
            Some(DiffCacheEntry::ExternalScan {
                project_id: "project-1".to_string(),
                worktrees: Vec::new(),
            }),
            failed_at + Duration::from_secs(2),
        )
        .is_empty());

    let failed_after_success = cache
        .claim_refresh(DiffCacheKey::ExternalScan("project-1".to_string()))
        .unwrap();
    assert_eq!(
        cache.publish_refresh(
            failed_after_success,
            Some(DiffCacheEntry::ExternalScanUnreadable {
                project_id: "project-1".to_string(),
            }),
            failed_at + Duration::from_secs(3),
        ),
        vec![CacheEffect::BoardChanged]
    );
}

#[test]
fn late_project_publication_does_not_recreate_a_removed_slot() {
    let mut cache = DiffCache::default();
    cache.register_project("project-1".to_string());
    let key = DiffCacheKey::ExternalScan("project-1".to_string());
    let claim = cache.claim_refresh(key.clone()).unwrap();
    cache.remove_project("project-1");
    assert!(cache.refresh_is_running(&key));

    assert!(cache
        .publish_refresh(
            claim,
            Some(DiffCacheEntry::ExternalScan {
                project_id: "project-1".to_string(),
                worktrees: Vec::new(),
            }),
            Instant::now(),
        )
        .is_empty());
    assert!(cache.external_scan("project-1").settled_at.is_none());
    assert!(cache.claim_refresh(key).is_some());
}

#[test]
fn workspace_membership_change_rejects_late_old_publication() {
    let mut cache = DiffCache::default();
    let old_repositories = vec!["/old".into()];
    let new_repositories = vec!["/new".into()];
    cache.sync_workspace_summaries(&[("workspace-1".to_string(), old_repositories.clone())]);
    let old_key =
        DiffCacheKey::WorkspaceSummary("workspace-1".to_string(), old_repositories.clone());
    let old_claim = cache.claim_refresh(old_key).unwrap();
    cache.sync_workspace_summaries(&[("workspace-1".to_string(), new_repositories.clone())]);
    let new_key =
        DiffCacheKey::WorkspaceSummary("workspace-1".to_string(), new_repositories.clone());
    let new_claim = cache.claim_refresh(new_key).unwrap();
    cache.publish_refresh(
        new_claim,
        Some(DiffCacheEntry::WorkspaceSummary {
            workspace_id: "workspace-1".to_string(),
            repositories: new_repositories.clone(),
            summary: json!({"pushes": 2}),
        }),
        Instant::now(),
    );

    assert!(cache
        .publish_refresh(
            old_claim,
            Some(DiffCacheEntry::WorkspaceSummary {
                workspace_id: "workspace-1".to_string(),
                repositories: old_repositories,
                summary: json!({"pushes": 1}),
            }),
            Instant::now(),
        )
        .is_empty());
    assert_eq!(
        cache
            .workspace_summary("workspace-1", &new_repositories)
            .unwrap()
            .value,
        &json!({"pushes": 2})
    );
}

#[test]
fn removed_workspace_rejects_late_summary_publication() {
    let mut cache = DiffCache::default();
    let repositories = vec!["/repo".into()];
    cache.sync_workspace_summaries(&[("workspace-1".to_string(), repositories.clone())]);
    let key = DiffCacheKey::WorkspaceSummary("workspace-1".to_string(), repositories.clone());
    let claim = cache.claim_refresh(key).unwrap();
    cache.sync_workspace_summaries(&[]);

    assert!(cache
        .publish_refresh(
            claim,
            Some(DiffCacheEntry::WorkspaceSummary {
                workspace_id: "workspace-1".to_string(),
                repositories: repositories.clone(),
                summary: json!({"pushes": 1}),
            }),
            Instant::now(),
        )
        .is_empty());
    assert!(cache
        .workspace_summary("workspace-1", &repositories)
        .is_none());
}

#[test]
fn invalidated_run_repopulation_is_not_a_file_change() {
    let mut cache = DiffCache::default();
    let key = DiffCacheKey::RunStat("run-1".to_string());
    let first = cache.claim_refresh(key.clone()).unwrap();
    cache.publish_refresh(
        first,
        Some(DiffCacheEntry::RunStat {
            run_id: "run-1".to_string(),
            stat: json!({"files_changed": 1}),
        }),
        Instant::now(),
    );
    cache.invalidate_run_stat("run-1");

    let next = cache.claim_refresh(key).unwrap();
    let effects = cache.publish_refresh(
        next,
        Some(DiffCacheEntry::RunStat {
            run_id: "run-1".to_string(),
            stat: json!({"files_changed": 2}),
        }),
        Instant::now(),
    );

    assert_eq!(effects, vec![CacheEffect::BoardChanged]);
    assert!(cache.run_files_changed_at("run-1").is_none());
}

#[test]
fn changed_run_stat_stays_uncommitted_until_notification_boundary() {
    let mut cache = DiffCache::default();
    let first_at = Instant::now();
    assert_eq!(
        cache.prepare_run_stat("run-1".to_string(), json!({"files_changed": 1}), first_at,),
        vec![CacheEffect::BoardChanged]
    );

    let changed_at = first_at + Duration::from_secs(1);
    let CachePublication::RunStatChanged(pending) =
        cache.prepare_run_stat("run-1".to_string(), json!({"files_changed": 2}), changed_at)
    else {
        panic!("changed stat must wait for its notification");
    };
    assert_eq!(cache.run_stat("run-1").unwrap().computed_at, first_at);
    assert_eq!(pending.run_id(), "run-1");

    cache.commit_changed_run_stat(pending);
    assert_eq!(cache.run_stat("run-1").unwrap().computed_at, changed_at);
    assert!(cache.run_files_changed_at("run-1").is_some());
}

#[test]
fn state_clock_moves_only_on_wire_state_transition() {
    let mut index = AttentionIndex::new(HashMap::new());
    assert!(index.observe_state("run-1", "building".to_string(), "one".to_string()));
    assert!(!index.observe_state("run-1", "building".to_string(), "two".to_string()));
    assert_eq!(
        index.clock("run-1").state_changed_at.as_deref(),
        Some("one")
    );
    assert!(index.observe_state("run-1", "review".to_string(), "three".to_string()));
    assert_eq!(
        index.clock("run-1").state_changed_at.as_deref(),
        Some("three")
    );
}

#[test]
fn stale_read_cursor_never_rewinds_and_legacy_applies_only_to_primary() {
    let mut index = AttentionIndex::new(HashMap::new());
    index.seed_legacy_read_cursor("issue-1", 7);
    index.mark_seen(
        "issue-1",
        None,
        &[("agent-1".to_string(), 10), ("agent-1".to_string(), 8)],
    );

    assert_eq!(index.read_cursor("issue-1", "agent-1", true), 10);
    assert_eq!(index.read_cursor("issue-1", "agent-2", true), 7);
    assert_eq!(index.read_cursor("issue-1", "agent-2", false), 0);
}

#[test]
fn dismissing_with_lower_and_omitted_lines_preserves_existing_agent_lines() {
    let mut index = AttentionIndex::new(HashMap::new());
    index.set_entity_dismissal(
        "issue-1",
        &[("agent-1".to_string(), 10), ("agent-2".to_string(), 7)],
    );

    index.set_entity_dismissal("issue-1", &[("agent-1".to_string(), 4)]);

    let attention = index.attention("issue-1").unwrap();
    assert_eq!(attention.dismissed_line_for("agent-1", true), Some(10));
    assert_eq!(attention.dismissed_line_for("agent-2", false), Some(7));
    assert!(attention.dismissal_active);
    assert!(attention.dismissal_tracks_messages);
}

#[test]
fn conversation_watermark_is_read_before_exact_replacement() {
    let mut index = AttentionIndex::new(HashMap::new());
    assert_eq!(index.conversation_watermark("thread-1").previous, None);
    assert!(index.advance_conversation("thread-1".to_string(), 8));
    assert_eq!(index.conversation_watermark("thread-1").previous, Some(8));
    assert!(!index.advance_conversation("thread-1".to_string(), 5));
    assert_eq!(index.conversation_watermark("thread-1").previous, Some(5));
}

#[test]
fn recovery_clock_restore_unconditionally_replaces_every_clock_fact() {
    let mut index = AttentionIndex::new(HashMap::new());
    index.record_created_if_absent("run-1", "old-created".to_string());
    index.record_updated("run-1", "old-updated".to_string());
    index.observe_state("run-1", "old-state".to_string(), "old-change".to_string());

    index.restore_entity_clocks(
        "run-1".to_string(),
        "stored-created".to_string(),
        "stored-updated".to_string(),
        "recovered-change".to_string(),
        "recovered-state".to_string(),
    );

    assert_eq!(
        index.clock("run-1"),
        super::EntityClock {
            created_at: Some("stored-created".to_string()),
            updated_at: Some("stored-updated".to_string()),
            state_changed_at: Some("recovered-change".to_string()),
        }
    );
    assert!(!index.observe_state(
        "run-1",
        "recovered-state".to_string(),
        "must-not-replace".to_string(),
    ));
}

#[test]
fn adopted_row_transfer_assigns_observed_time_and_optional_message_dismissal() {
    let mut index = AttentionIndex::new(HashMap::new());
    index.observe_row("run-1", "old-observed");

    index.transfer_adopted_row("run-1", Some("row-observed".to_string()), true);

    let attention = index.attention("run-1").unwrap();
    assert_eq!(attention.first_observed_at.as_deref(), Some("row-observed"));
    assert!(attention.dismissal_active);
    assert!(attention.dismissal_tracks_messages);
}

#[test]
fn replacing_loaded_attention_entries_preserves_clocks_and_watermarks() {
    let mut index = AttentionIndex::new(HashMap::new());
    index.restore_entity_clocks(
        "run-1".to_string(),
        "created".to_string(),
        "updated".to_string(),
        "changed".to_string(),
        "review".to_string(),
    );
    index.seed_conversation("thread-1".to_string(), 12);
    let mut loaded = HashMap::new();
    loaded.insert("run-1".to_string(), Default::default());

    index.replace_entries(loaded);

    assert!(index.attention("run-1").is_some());
    assert_eq!(index.clock("run-1").created_at.as_deref(), Some("created"));
    assert_eq!(index.conversation_watermark("thread-1").previous, Some(12));
}

#[test]
fn anchor_seed_reports_only_the_first_seed() {
    let mut index = AttentionIndex::new(HashMap::new());

    assert!(index.seed_anchor("run-1", "created"));
    assert!(!index.seed_anchor("run-1", "must-not-replace"));
    assert_eq!(
        index.attention("run-1").unwrap().anchor_at.as_deref(),
        Some("created")
    );
}
