//! Explicit registry coverage; entries are not claims of executed success.
use super::*;
use crate::api::effects::{effect, Effect};
use std::collections::BTreeSet;

const EXEMPT_WRITES: &[&str] = &[
    "session.hello",
    "changes.subscribe",
    "changes.unsubscribe",
    "term.attach",
    "agent.attach",
    "term.input",
    "term.resize",
    "term.ack",
    "stream.start",
];

// Each family requires a finite-service recipe and a retained completion proof.
const WRITE_FAMILIES: &[(&str, &[&str])] = &[
    (
        "agent",
        &[
            "agent.add",
            "agent.choose",
            "agent.interrupt",
            "agent.remove",
            "agent.start",
        ],
    ),
    ("branch", &["branch.dispatch", "branch.finish"]),
    ("bridge", &["bridge.check_update", "bridge.install_update"]),
    (
        "capture",
        &[
            "capture.answer",
            "capture.cancel",
            "capture.create",
            "capture.reroute",
        ],
    ),
    (
        "conversation",
        &[
            "conversation.reset",
            "conversation.settings",
            "conversation.unwatch",
            "conversation.watch",
        ],
    ),
    ("entity", &["entity.dismiss", "entity.mute", "entity.seen"]),
    (
        "fs",
        &[
            "fs.createDirectory",
            "fs.createFile",
            "fs.mkdir",
            "fs.uploadAbort",
            "fs.uploadBegin",
            "fs.uploadChunk",
            "fs.uploadFinish",
            "fs.write",
        ],
    ),
    (
        "git",
        &[
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
        ],
    ),
    ("harnesses", &["harnesses.refresh"]),
    (
        "project",
        &[
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
        ],
    ),
    ("push", &["push.registerKey", "push.revokeKey"]),
    (
        "run",
        &[
            "run.abandon",
            "run.adopt",
            "run.git_action",
            "run.message",
            "run.request_changes",
        ],
    ),
    ("settings", &["settings.set"]),
    (
        "tasks",
        &[
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
        ],
    ),
    ("term", &["term.close", "term.create"]),
    ("thread", &["thread.attach", "thread.post"]),
    ("user", &["user.present"]),
    (
        "workspace",
        &[
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
        ],
    ),
];

fn registered_methods() -> BTreeSet<&'static str> {
    crate::api::v1::methods()
        .iter()
        .map(|(name, _)| *name)
        .chain(crate::api::LEGACY_METHODS.iter().copied())
        .chain(crate::api::QA_METHODS.iter().copied())
        .collect()
}

#[test]
fn every_deployed_write_has_an_explicit_semantic_family_or_exemption() {
    let writes: BTreeSet<_> = registered_methods()
        .into_iter()
        .filter(|method| effect(method) == Some(Effect::Write))
        .collect();
    let entries: Vec<_> = WRITE_FAMILIES
        .iter()
        .flat_map(|(_, methods)| methods.iter().copied())
        .chain(EXEMPT_WRITES.iter().copied())
        .collect();
    let classified: BTreeSet<_> = entries.iter().copied().collect();
    assert_eq!(
        entries.len(),
        classified.len(),
        "duplicate family or exemption"
    );
    assert_eq!(
        writes, classified,
        "new writes require an audited finite-service recipe"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_semantic_method_has_a_finite_service_recipe_before_qualification() {
    crate::git_fixture::environment::isolated_git_test!();
    let verified = super::rare::execute_all().await;
    let expected: BTreeSet<_> = WRITE_FAMILIES
        .iter()
        .flat_map(|(_, methods)| methods.iter().copied())
        .collect();
    let observed: BTreeSet<_> = verified.iter().map(|step| step.method.as_str()).collect();
    assert_eq!(
        expected, observed,
        "every semantic method requires real wire success and effect proof"
    );
    assert!(!verified.is_empty());
}

#[test]
fn qualification_load_counts_exempt_frames_separately_from_semantic_writes() {
    assert_eq!(CALLERS, 12);
    assert_eq!(TRIAL_SECONDS * WRITES_PER_SECOND, 24_000);
    assert_eq!(TRIAL_SECONDS * EXEMPT_PER_SECOND, 60_000);
    assert_eq!(EXEMPT_WRITES.len(), 9);
    assert!(!EXEMPT_WRITES.contains(&"thread.post"));
    assert!(!EXEMPT_WRITES.contains(&"tasks.comment"));
    assert!(!EXEMPT_WRITES.contains(&"term.create"));
}
