//! Native read requests and saved-value requirements.
//! No response producer or generic success fallback exists here.
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone)]
pub(in crate::app::tests::runtime::write_wait_qualification) struct Inputs {
    pub project: String,
    pub source: String,
    pub repository: PathBuf,
    pub workspace: String,
    pub owner: String,
    pub agent: String,
    pub task: String,
    pub dependent: String,
    pub label: String,
    pub operation: String,
    pub activity_from: u64,
    pub activity_through: u64,
    pub revision: String,
    pub capture: String,
    pub stream: String,
    pub ordinary_plan: String,
    pub staged_plan: String,
    pub stage: String,
    pub stage_run: String,
    pub archived_run: String,
    pub external_worktree: String,
    pub commit: String,
    pub stable_file: String,
    pub diff_file: String,
    pub snapshot: String,
    pub review_directory: String,
    pub task_attachment: String,
    pub thread_attachment: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::tests::runtime::write_wait_qualification) enum Form {
    Ready,
    LegacyFull,
}

pub(in crate::app::tests::runtime::write_wait_qualification) struct Case {
    pub method: &'static str,
    pub form: Form,
    pub params: Value,
    /// Exact requirements for the native seed's known witness, in addition to
    /// api::v1::check_read_result typed shape validation. These are not outputs.
    pub oracle: &'static str,
}

pub(in crate::app::tests::runtime::write_wait_qualification) fn cases(i: &Inputs) -> Vec<Case> {
    let scope = json!({"workspace_id":i.workspace,"source_id":i.source});
    let scoped = |extra: Value| {
        let mut value = scope.clone();
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        value
    };
    let address = json!({"entity_id":i.owner,"agent_id":i.agent});
    let addressed = |extra: Value| {
        let mut value = address.clone();
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        value
    };
    let review = json!({"task_id":i.task,"snapshot_id":i.snapshot,
        "directory_id":i.review_directory,"mode":"blob","path":i.stable_file,
        "range":{"offset":0,"bytes":4096}});
    let ready = |method, params, oracle| Case {
        method,
        params,
        oracle,
        form: Form::Ready,
    };
    let legacy = |method, params, oracle| Case {
        method,
        params,
        oracle,
        form: Form::LegacyFull,
    };
    let mut cases = vec![
        ready("thread.page", addressed(json!({"after_sequence":0,"limit":20})),
            "items contains reserved message/activity ID+body; correct canonical thread identity; has_more/sequence bounds; page size<=20"),
        ready("thread.activity", addressed(json!({"from_sequence":i.activity_from,"through_sequence":i.activity_through,"limit":20})),
            "items contains actual reserved activity ID+payload; sequence lies in requested inclusive interval"),
        ready("thread.operation", addressed(json!({"operation_id":i.operation})),
            "operation_id/method/entity_id/agent_id/conversation_id/posted_sequence/status equal actual saved final receipt; queued/accepted is not delivered"),
        ready("tasks.list", json!({"project_id":i.project,"label":i.label,"limit":20}),
            "project_id exact; tasks contains reserved task ID+immutable label/title/project; <=20; cursor shape valid"),
        ready("tasks.get", json!({"task_id":i.task}),
            "task.id/project_id/immutable title exact; timeline contains seeded comment ID+body+native attachment descriptor"),
        ready("tasks.waiting_on.blocks", json!({"task_id":i.task,"limit":20}),
            "blocks contains known dependent task ID+number, state and resolved semantics; bound<=20"),
        ready("agent.list", json!({"entity_id":i.owner}),
            "agents contains real canonical agent ID/conversation ID/ordinal/provider; accepted owner selection; not empty fallback"),
        ready("board.list", json!({}),
            "items contains reserved task ID+project and workspace row; captures fixture appears only in intended RPC board; verify runs/workspaces arrays typed"),
        ready("project.list", json!({}),
            "projects contains all four exact project IDs/native source identities and current accepted paths"),
        ready("settings.get", json!({}),
            "known explicit fixture setting/default harness and catalogue role model survive; version/feature shapes are valid"),
        ready("workspace.list", json!({"project_id":i.project}),
            "workspaces contains known workspace_id+project_id; no foreign project rows"),
        ready("workspace.get", json!({"workspace_id":i.workspace}),
            "workspace_id/project_id, directories source_id/path, selected conversation/agent ID exact; not pending/null"),
        ready("capture.get", json!({"capture_id":i.capture}),
            "id/text equal actual durable capture; expected state/routing/question shape; compare Store capture record"),
        ready("models.list", json!({}),
            "known fake installed provider/model ID/context window/version; explicit not-installed sibling; no host probe"),
        ready("harnesses.list", json!({}),
            "known fake installed and not-installed harness rows; fresh inventory checked timestamp; requested==completed sweep; no fabricated running-version evidence"),
        ready("bridge.update_status", json!({}),
            "running_version/platform/current state and fixed release tag from fake UpdateBackend; real service check completed"),
        ready("task.stages", json!({"task_id":i.staged_plan}),
            "plan/task ID exact; stages contains known stage ID/path/title; completed SHA/publication plus separate pending stage exact"),
        ready("tasks.columns", json!({"project_id":i.project}),
            "known configured project column slugs and order; nonempty native default/custom columns"),
        ready("thread.revision", addressed(json!({"revision_id":i.revision})),
            "revision_id/artifact exact; contents equal retained revision snapshot bytes, not only summary"),
        ready("stream.events", json!({"stream_id":i.stream,"since":0,"limit":20}),
            "all real output events and terminal done, dense seq, next=head=9 and complete; known chunk text"),
        ready("stream.state", json!({"stream_id":i.stream}),
            "stream_id/count8/head9/complete true/output_len/exact SHA256 checksum match native eight-output stream"),
        ready("git.log", scoped(json!({"limit":20,"skip":0})),
            "commits contains retained seed commit SHA+subject, no fabricated ref; valid newest/branch/more/reset"),
        ready("git.show", scoped(json!({"hash":i.commit,"range":{"offset":0,"bytes":4096}})),
            "hash/subject exact, patch contains actual known committed file/body, range version+offset/total coherent"),
        ready("git.status", scoped(json!({})),
            "path/head/native branch exact; known dedicated dirty file row+status_key/stat; full fresh shape, not conditional unchanged"),
        ready("git.diff", scoped(json!({"paths":[i.diff_file],"range":{"offset":0,"bytes":4096}})),
            "named file row and actual known hunk/changed bytes; patch page bound; no unrelated body"),
        ready("git.changeset_diff", scoped(json!({"paths":[i.diff_file],"range":{"offset":0,"bytes":4096}})),
            "actual known file row/hunk from same source changeset, stable diff_key and range witness"),
        ready("git.refs", scoped(json!({})),
            "current local branch full_ref and seeded upstream exact; one ahead and zero behind"),
        ready("git.unpushed", scoped(json!({"patch":false})),
            "native seeded unpublished commit/file/stat/diff_key + base; patch omitted by explicit request; not empty no-upstream fallback"),
        ready("worktree.diff", json!({"project_id":i.project,"worktree_id":i.external_worktree,"patch":false}),
            "known actual observed worktree_id and branch; base/stat/files/diff_key; path belongs to fixture"),
        ready("run.diff", json!({"run_id":i.owner,"patch":false}),
            "known actual run baseline diff has dedicated file/stat/diff_key; not an empty no-baseline fallback"),
        ready("task.stage_diff", json!({"task_id":i.staged_plan,"stage_id":i.stage}),
            "actual run/stage/task IDs and pinned start/completion SHAs; known immutable hunk; unavailable is separate class, not ready"),
        ready("fs.list", json!({"path":i.repository.to_str().unwrap()}),
            "absolute fixture path and known child directory name/path; no home directory fallback"),
        ready("fs.tree", scoped(json!({"path":""})),
            "entries contains known stable file by name+kind+size and fixture subdirectory; scope path exact"),
        ready("fs.read", scoped(json!({"path":i.stable_file,"range":{"offset":0,"bytes":4096}})),
            "path/size/content_b64 decoded bytes equal known fixture file, offset/end/total/version coherent"),
        ready("github.repos", json!({}),
            "repos contains exact native fake gh name_with_owner/ssh_url/url/private; invocation record proves dedicated temporary executable"),
        legacy("task.doc", json!({"task_id":i.ordinary_plan}),
            "plan_path and full contents equal actual Store::plan_docs_dir file for ordinary legacy plan"),
        legacy("task.stage_doc", json!({"task_id":i.staged_plan,"stage_id":i.stage}),
            "task_id/plan_id/stage_id/path exact; contents equal actual staged document file"),
        ready("tasks.review.get", json!({"task_id":i.task}),
            "review.task_id/version; snapshots contains real saved snapshot+directory IDs and source pin; review:null is not populated ready fixture"),
        ready("tasks.review.diff", review,
            "saved blob decoded bytes equal snapshot README, editable false, version actual blob SHA; text page clamp honored"),
        ready("tasks.attachment", json!({"task_id":i.task,"path":i.task_attachment,"offset":0,"length":4096}),
            "returned path/offset/size/mime and exact decoded native attachment page; task actually references descriptor"),
        ready("thread.attachment", addressed(json!({"path":i.thread_attachment,"offset":0,"length":4096})),
            "returned path/offset/size/mime and exact decoded native thread attachment page; canonical generation fence retained"),
        ready("workspace.git_init_options", json!({"workspace_id":i.workspace,"source_id":i.source}),
            "workspace_id/source_id/native paths; actual git directory yields correct source/workspace available/is_git flags"),
        legacy("archived.list", json!({}),
            "items contains known existing archived run ID+project/terminal state; count resident history of archived record"),
        legacy("task.get", json!({"task_id":i.ordinary_plan,"thread_limit":20}),
            "plan/task ID+goal, actual canonical agents/thread window, explicit<=20 history rows; legacy method remains separately reported"),
        legacy("term.list", scoped(json!({})),
            "terminals contains retained fake SHELL term_id+cols/rows/created_at; fake agent tab must not satisfy shell witness"),
    ];
    const PREFIX: [&str; 9] = [
        "tasks.get",
        "tasks.waiting_on.blocks",
        "tasks.review.get",
        "tasks.review.diff",
        "tasks.attachment",
        "thread.attachment",
        "task.doc",
        "task.stage_doc",
        "capture.get",
    ];
    cases.sort_by_key(|case| {
        PREFIX
            .iter()
            .position(|method| *method == case.method)
            .unwrap_or(PREFIX.len())
    });
    cases
}

pub(in crate::app::tests::runtime::write_wait_qualification) fn assert_registry(cases: &[Case]) {
    use crate::api::effects::{effect, Effect};
    let expected: BTreeSet<_> = crate::api::v1::methods()
        .iter()
        .map(|(m, _)| *m)
        .chain(crate::api::LEGACY_METHODS.iter().copied())
        .chain(crate::api::QA_METHODS.iter().copied())
        .filter(|m| {
            matches!(
                effect(m),
                Some(
                    Effect::SqlRead
                        | Effect::SnapshotRead
                        | Effect::ExternalRead
                        | Effect::LegacyRead
                )
            )
        })
        .collect();
    let actual: BTreeSet<_> = cases.iter().map(|c| c.method).collect();
    assert_eq!(
        actual.len(),
        cases.len(),
        "duplicate ready/primary case: separately keyed variants belong in another form registry"
    );
    assert_eq!(actual, expected, "a deployed read must have a real recipe");
}

pub(in crate::app::tests::runtime::write_wait_qualification) use super::rpc::ReadOutcome as Outcome;

fn classify(case: &Case, reply: &Value) -> Outcome {
    super::rpc::classify(case.method, reply)
}

mod oracles;
pub(in crate::app::tests::runtime::write_wait_qualification) use oracles::{
    assert_reply, Witnesses,
};
