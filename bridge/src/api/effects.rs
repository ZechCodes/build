//! Audited effects of every RPC entry. Migrated immutable snapshots execute
//! at intake; SQL reads use dedicated workers, and filesystem/Git observations
//! use an independent bounded I/O lane.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effect {
    SqlRead,
    Control,
    SnapshotRead,
    ExternalRead,
    LegacyRead,
    Write,
}

pub const SQL_READ_METHODS: &[&str] = &[
    "thread.page",
    "thread.activity",
    "thread.operation",
    "thread.revision",
    "tasks.list",
    "tasks.get",
    "tasks.waiting_on.blocks",
];

/// Families whose hidden observation work is owned by publication producers.
pub const INTAKE_SNAPSHOT_METHODS: &[&str] = &[
    "board.list",
    "project.list",
    "workspace.list",
    "workspace.get",
    "agent.list",
    "settings.get",
    "models.list",
    "harnesses.list",
    "bridge.update_status",
    "capture.get",
    "task.get",
    "task.stages",
    "tasks.columns",
    "stream.events",
    "stream.state",
];

pub(crate) fn is_snapshot_read(payload: &Value) -> bool {
    payload
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|method| INTAKE_SNAPSHOT_METHODS.contains(&method))
}

const CONTROL_METHODS: &[&str] = &["ping", "bridge.stats", "rtc.offer", "rtc.ice", "rtc.close"];

const SNAPSHOT_METHODS: &[&str] = &[
    "agent.list",
    "board.list",
    "project.list",
    "settings.get",
    "workspace.list",
    "workspace.get",
    "capture.get",
    "models.list",
    "harnesses.list",
    "bridge.update_status",
    "task.get",
    "task.stages",
    "tasks.columns",
    "stream.events",
    "stream.state",
];

const EXTERNAL_READ_METHODS: &[&str] = &[
    "git.log",
    "git.show",
    "git.status",
    "git.diff",
    "git.changeset_diff",
    "git.refs",
    "git.unpushed",
    "worktree.diff",
    "run.diff",
    "task.stage_diff",
    "fs.list",
    "fs.tree",
    "fs.read",
    "github.repos",
    "task.doc",
    "task.stage_doc",
    "tasks.review.get",
    "tasks.review.diff",
    "tasks.attachment",
    "thread.attachment",
    "workspace.git_init_options",
];

const LEGACY_READ_METHODS: &[&str] = &["archived.list", "term.list"];

const WRITE_METHODS: &[&str] = &[
    "agent.add",
    "agent.attach",
    "agent.choose",
    "agent.interrupt",
    "agent.remove",
    "agent.start",
    "branch.dispatch",
    "branch.finish",
    "bridge.check_update",
    "bridge.install_update",
    "capture.answer",
    "capture.cancel",
    "capture.create",
    "capture.reroute",
    "changes.subscribe",
    "changes.unsubscribe",
    "conversation.reset",
    "conversation.settings",
    "conversation.unwatch",
    "conversation.watch",
    "entity.dismiss",
    "entity.mute",
    "entity.seen",
    "fs.createDirectory",
    "fs.createFile",
    "fs.mkdir",
    "fs.uploadAbort",
    "fs.uploadBegin",
    "fs.uploadChunk",
    "fs.uploadFinish",
    "fs.write",
    "git.checkout_ref",
    "git.commit",
    "git.discard",
    "git.fetch",
    "git.merge_abort",
    "git.pull",
    "git.push",
    "git.stage",
    "git.stash",
    "git.stash_pop",
    "harnesses.refresh",
    "project.add_source",
    "project.create",
    "project.delete",
    "project.ensure_conversation",
    "project.init_git",
    "project.remove_source",
    "project.set_compaction",
    "project.set_isolation",
    "project.set_remote",
    "project.sync_source",
    "project.update_source",
    "push.registerKey",
    "push.revokeKey",
    "run.abandon",
    "run.adopt",
    "run.git_action",
    "run.message",
    "run.request_changes",
    "session.hello",
    "settings.set",
    "stream.start",
    "tasks.assign",
    "tasks.attach",
    "tasks.close",
    "tasks.comment",
    "tasks.create",
    "tasks.read_through",
    "tasks.reopen",
    "tasks.review.act",
    "tasks.review.close",
    "tasks.review.complete",
    "tasks.review.merge",
    "tasks.review.open",
    "tasks.review.push",
    "tasks.review.refresh",
    "tasks.review.reopen",
    "tasks.review.snapshot",
    "tasks.review.update",
    "tasks.unwatch",
    "tasks.update",
    "tasks.waiting_on.add",
    "tasks.waiting_on.remove",
    "tasks.watch",
    "term.ack",
    "term.attach",
    "term.close",
    "term.create",
    "term.input",
    "term.resize",
    "thread.attach",
    "thread.post",
    "user.present",
    "workspace.add_directory",
    "workspace.create",
    "workspace.delete",
    "workspace.ensure_conversation",
    "workspace.finish",
    "workspace.init_git",
    "workspace.measure_sizes",
    "workspace.reclaim",
    "workspace.remove_directory",
    "workspace.rename",
    "workspace.retry",
    "workspace.set_compaction",
    "workspace.set_locked",
];

pub(crate) fn effect(method: &str) -> Option<Effect> {
    [
        (SQL_READ_METHODS, Effect::SqlRead),
        (CONTROL_METHODS, Effect::Control),
        (SNAPSHOT_METHODS, Effect::SnapshotRead),
        (EXTERNAL_READ_METHODS, Effect::ExternalRead),
        (LEGACY_READ_METHODS, Effect::LegacyRead),
        (WRITE_METHODS, Effect::Write),
    ]
    .into_iter()
    .find_map(|(methods, effect)| methods.contains(&method).then_some(effect))
}

/// External observations use an independent bounded I/O lane. Archive reads
/// also observe checkout existence despite their legacy classification.
pub(crate) fn is_observation_read(payload: &Value) -> bool {
    payload
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|method| EXTERNAL_READ_METHODS.contains(&method) || method == "archived.list")
}

pub(crate) fn observation_needs_sql(payload: &Value) -> bool {
    matches!(
        payload.get("method").and_then(Value::as_str),
        Some("tasks.review.get" | "tasks.review.diff" | "tasks.attachment")
    )
}

pub(crate) fn is_sql_read(payload: &Value) -> bool {
    payload
        .get("method")
        .and_then(Value::as_str)
        .and_then(effect)
        == Some(Effect::SqlRead)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_registered_legacy_and_qa_entry_has_one_effect() {
        let registered: BTreeSet<_> = crate::api::v1::methods()
            .iter()
            .map(|(name, _)| *name)
            .chain(crate::api::LEGACY_METHODS.iter().copied())
            .chain(crate::api::QA_METHODS.iter().copied())
            .collect();
        let entries: Vec<_> = [
            SQL_READ_METHODS,
            CONTROL_METHODS,
            SNAPSHOT_METHODS,
            EXTERNAL_READ_METHODS,
            LEGACY_READ_METHODS,
            WRITE_METHODS,
        ]
        .into_iter()
        .flatten()
        .copied()
        .collect();
        let classified: BTreeSet<_> = entries.iter().copied().collect();
        assert_eq!(entries.len(), classified.len(), "duplicate effects");
        assert_eq!(registered, classified, "audit new or removed RPC entries");
        assert_eq!(effect("made.up"), None);
    }
}
