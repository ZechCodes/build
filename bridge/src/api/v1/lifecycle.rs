//! The lifecycle family: `issue.*`, `plan.*`, `run.*`, `branch.*`,
//! `worktree.create`, `worktree.finish`, `entity.*`, `triage.override`.
//! (The diff reads — `run.diff`, `run.stage_diff`, `issue.diff`,
//! `issue.stage_diff` — are the git family's.)
//!
//! Two halves, converted independently and kept apart on purpose: the
//! `issue.*` / `plan.*` half below, and the `run.*` / `branch.*` /
//! `worktree.*` half under the section marker at the bottom. Append to your
//! own section; never reorder the other's.
//!
//! `plan.*` is the deprecated alias of `issue.*` (spec step 2.1: the kind of
//! thing a major retires). An alias pair is ONE typed handler: [`IssueRef`]
//! reads either spelling and writes both back out, so the plan-store
//! implementation underneath — which still names the id `plan_id` — keeps
//! reading exactly what it always read, and the canonical `issue_id` is what
//! the contract states.

use super::{answer, deferral_placeholder, Answer, Handler, NoParams, WireParams};
use crate::api::ApiError;
use crate::app::AppState;
use crate::{v1_method, v1_methods};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};

/// The verbs this family serves.
pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        // ---- issue/plan
        v1_method!("issue.create", issue_create, IssueCreateParams, IssueView),
        v1_method!("plan.create", issue_create, IssueCreateParams, IssueView),
        v1_method!("issue.get", issue_get, IssueDetailParams, IssueView),
        v1_method!("plan.get", issue_get, IssueDetailParams, IssueView),
        v1_method!("issue.list", issue_list, NoParams, IssueListResult),
        v1_method!("plan.list", plan_list, NoParams, PlanListResult),
        v1_method!("issue.doc", issue_doc, IssueRefParams, IssueDocResult),
        v1_method!("plan.doc", issue_doc, IssueRefParams, IssueDocResult),
        v1_method!(
            "issue.stages",
            issue_stages,
            IssueRefParams,
            IssueStagesResult
        ),
        v1_method!("plan.stages", plan_stages, IssueRefParams, PlanStagesResult),
        v1_method!(
            "issue.stage_doc",
            issue_stage_doc,
            StageRefParams,
            StageDocResult
        ),
        v1_method!(
            "plan.stage_doc",
            issue_stage_doc,
            StageRefParams,
            StageDocResult
        ),
        v1_method!(
            "issue.approve",
            issue_approve,
            IssueMutationParams,
            IssueView
        ),
        v1_method!(
            "plan.approve",
            issue_approve,
            IssueMutationParams,
            IssueView
        ),
        v1_method!(
            "issue.send_notes",
            issue_send_notes,
            SendNotesParams,
            IssueView
        ),
        v1_method!(
            "plan.send_notes",
            issue_send_notes,
            SendNotesParams,
            IssueView
        ),
        v1_method!(
            "issue.stage_approve",
            issue_stage_approve,
            StageMutationParams,
            IssueView
        ),
        v1_method!(
            "plan.stage_approve",
            issue_stage_approve,
            StageMutationParams,
            IssueView
        ),
        v1_method!(
            "issue.stage_revise",
            issue_stage_revise,
            StageMutationParams,
            IssueView
        ),
        v1_method!(
            "plan.stage_send_notes",
            issue_stage_revise,
            StageMutationParams,
            IssueView
        ),
        v1_method!("plan.message", issue_message, IssueMessageParams, IssueView),
        v1_method!(
            "plan.abandon",
            issue_abandon,
            IssueMutationParams,
            IssueView
        ),
        v1_method!(
            "issue.comment_add",
            issue_comment_add,
            CommentAddParams,
            CommentAddResult
        ),
        v1_method!(
            "plan.comment_add",
            issue_comment_add,
            CommentAddParams,
            CommentAddResult
        ),
        v1_method!(
            "issue.comment_delete",
            issue_comment_delete,
            CommentDeleteParams,
            Acknowledged
        ),
        v1_method!(
            "plan.comment_delete",
            issue_comment_delete,
            CommentDeleteParams,
            Acknowledged
        ),
        v1_method!(
            "issue.archive",
            issue_archive,
            IssueMutationParams,
            IssueView
        ),
        v1_method!(
            "plan.archive",
            issue_archive,
            IssueMutationParams,
            IssueView
        ),
        v1_method!("issue.delete", issue_delete, IssueRefParams, Acknowledged),
        v1_method!("plan.delete", issue_delete, IssueRefParams, Acknowledged),
        v1_method!(
            "issue.implement_stage",
            issue_implement_stage,
            ImplementStageParams,
            IssueView
        ),
        v1_method!(
            "issue.implement_all",
            issue_implement_all,
            ImplementAllParams,
            IssueView
        ),
        v1_method!(
            "issue.set_auto_advance",
            issue_set_auto_advance,
            SetAutoAdvanceParams,
            IssueView
        ),
        v1_method!(
            "issue.stage_fix",
            issue_stage_fix,
            StageFixParams,
            IssueView
        ),
        v1_method!(
            "issue.request_changes",
            issue_request_changes,
            RequestChangesParams,
            IssueView
        ),
        v1_method!(
            "issue.git_action",
            issue_git_action,
            IssueGitActionParams,
            IssueView
        ),
        v1_method!("entity.seen", entity_seen, EntitySeenParams, Acknowledged),
        v1_method!("entity.mute", entity_mute, EntityMuteParams, MuteResult),
        v1_method!(
            "entity.dismiss",
            entity_dismiss,
            EntityDismissParams,
            DismissResult
        ),
        v1_method!(
            "triage.override",
            triage_override,
            TriageOverrideParams,
            TriageOverrideResult
        ),
        // ---- end issue/plan (the run/branch/worktree half appends below)
        // ---- run/branch/worktree
        v1_method!("run.create", run_create, RunCreateParams, RunView),
        v1_method!("run.get", run_get, RunViewParams, RunView),
        v1_method!(
            "run.request_changes",
            run_request_changes,
            RunRequestChangesParams,
            RunView
        ),
        v1_method!(
            "run.stage_dispatch",
            run_stage_dispatch,
            RunStageDispatchParams,
            RunView
        ),
        v1_method!("run.stage_fix", run_stage_fix, RunStageFixParams, RunView),
        v1_method!(
            "run.stage_send_notes",
            run_stage_send_notes,
            RunStageParams,
            RunView
        ),
        v1_method!(
            "run.set_auto_advance",
            run_set_auto_advance,
            RunAutoAdvanceParams,
            RunView
        ),
        v1_method!(
            "run.git_action",
            run_git_action,
            RunGitActionParams,
            RunView
        ),
        v1_method!("run.message", run_message, RunMessageParams, RunView),
        v1_method!("run.abandon", run_abandon, RunViewParams, RunView),
        v1_method!("run.delete", run_delete, RunIdParams, RunAck),
        v1_method!("run.adopt", run_adopt, RunAdoptParams, RunAdoptResult),
        v1_method!("run.release", run_release, RunIdParams, RunAck),
        v1_method!(
            "run.finish",
            run_finish,
            RunFinishParams,
            WorkspaceFinishResult
        ),
        v1_method!("branch.get", branch_get, BranchGetParams, BranchWorkItem),
        v1_method!(
            "branch.dispatch",
            branch_dispatch,
            BranchDispatchParams,
            DispatchedAgentResult
        ),
        v1_method!(
            "branch.finish",
            branch_finish,
            BranchFinishParams,
            WorkspaceFinishResult
        ),
        v1_method!(
            "worktree.create",
            worktree_create,
            WorktreeCreateParams,
            CreatedWorktreeResult
        ),
        v1_method!(
            "worktree.finish",
            worktree_finish,
            WorktreeFinishParams,
            WorkspaceFinishResult
        ),
        // ---- end run/branch/worktree
    ]
}

// ------------------------------------------------ issue/plan: shared wire ---

/// A page size, exactly as its client spelled it: a JSON integer, the whole
/// float a language with no integer type encodes one as, or the string a URL
/// left behind. Anything unreadable still means "this client can page" and is
/// answered with the default page rather than the whole conversation, so the
/// value is carried through verbatim rather than parsed here — that decision
/// belongs to one place, and it is not the facade.
///
/// Absent and `null` both mean the conversation whole.
pub type ThreadLimit = serde_json::Value;

/// A conversation cursor, exactly as its client spelled it. A cursor nobody
/// can read is treated as absent — the poll gets the page it asked for rather
/// than a refusal — so, like [`ThreadLimit`], it is carried through rather
/// than parsed here.
pub type ThreadCursor = serde_json::Value;

/// The conversation embedded in an issue or run view: the thread family's
/// items, sessions and revisions without `thread.page`'s paging envelope.
/// Carried verbatim until the thread family names the embedded form.
pub type ThreadPayload = serde_json::Value;

/// One agent's bubble, as `agent.list` renders it — the thread family's
/// shape, named once, there.
pub type AgentDigest = crate::api::v1::thread::AgentDigest;

/// The issue a verb acts on.
///
/// Reads either spelling — `issue_id` (canonical) or `plan_id` (the `plan.*`
/// alias) — and writes BOTH back to the implementation underneath, which is
/// what the legacy route's `alias_param` did by hand at each call site.
#[derive(Debug, Deserialize)]
pub struct IssueRef {
    #[serde(alias = "plan_id")]
    pub issue_id: String,
}

impl Serialize for IssueRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("issue_id", &self.issue_id)?;
        map.serialize_entry("plan_id", &self.issue_id)?;
        map.end()
    }
}

/// How much conversation a read may carry back, and whose.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ThreadWindowParams {
    /// The agent whose conversation to answer with; the entity's own when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// A stale-binding guard: refused when it is not what the agent is bound
    /// to now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Only what happened past this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_after_sequence: Option<ThreadCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<ThreadLimit>,
}

/// One reviewer message in a batch: what was said, and where they were
/// standing when they said it.
#[derive(Debug, Deserialize, Serialize)]
pub struct ReviewerMessageParams {
    /// Required, non-empty, at most 32000 bytes — checked by the
    /// implementation, which is where the batch's other rules live too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Where in the artifact the message is anchored. The artifact decides the
    /// shape (a doc passage for a plan, a hunk for a diff), so it is carried
    /// verbatim to the parser that knows which one this is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewing_context: Option<crate::thread::ViewingContext>,
}

// ----------------------------------------------------- issue/plan: params ---

#[derive(Debug, Deserialize, Serialize)]
pub struct IssueRefParams {
    #[serde(flatten)]
    pub issue: IssueRef,
}

/// A mutation that answers with the issue: the id, and how much of the
/// conversation the answer carries.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueMutationParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<ThreadLimit>,
}

/// The detail poll: the whole issue, with the conversation window the surface
/// is holding.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueDetailParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    #[serde(flatten)]
    pub thread: ThreadWindowParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IssueCreateParams {
    /// What the issue is for, in the human's own words.
    pub goal: String,
    /// The account's default project when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// `false` files the record and starts nothing — an inert issue, whose
    /// first `thread.post` opens the planning session. Defaults to `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<bool>,
    /// The harness to plan with; the account's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<ThreadLimit>,
}

/// A read of one stage's document.
#[derive(Debug, Deserialize, Serialize)]
pub struct StageRefParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub stage_id: String,
}

/// A mutation aimed at one stage.
#[derive(Debug, Deserialize, Serialize)]
pub struct StageMutationParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub stage_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<ThreadLimit>,
}

/// Notes back to the issue's agent: a batch of anchored messages, or the one
/// unanchored body the pre-batch clients send.
#[derive(Debug, Deserialize, Serialize)]
pub struct SendNotesParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<ReviewerMessageParams>>,
    /// The single-body spelling, required when `messages` is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewing_context: Option<crate::thread::ViewingContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<ThreadLimit>,
}

/// A freeform human message to the issue's agent.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueMessageParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewing_context: Option<crate::thread::ViewingContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<ThreadLimit>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CommentAddParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub stage_id: String,
    pub body: String,
    /// Absent for a comment on the document as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<crate::thread::DocAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewing_context: Option<crate::thread::ViewingContext>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CommentDeleteParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub comment_id: String,
}

/// Open — or carry on — an issue's implementation.
///
/// The checkout is chosen here or not at all: `worktree_id` implements into a
/// checkout that already exists (which adopts it), and `base_branch` names
/// what a checkout cut for this issue is cut from. Naming any of
/// `provider`/`model`/`effort` overrides the issue's own choice for the
/// implementation agent.
#[derive(Debug, Deserialize, Serialize)]
pub struct ImplementAllParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<ThreadLimit>,
}

/// The same, aimed at one approved stage rather than the whole manifest.
#[derive(Debug, Deserialize, Serialize)]
pub struct ImplementStageParams {
    #[serde(flatten)]
    pub implement: ImplementAllParams,
    pub stage_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SetAutoAdvanceParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<ThreadLimit>,
}

/// Send one stage back to its builder with a note.
#[derive(Debug, Deserialize, Serialize)]
pub struct StageFixParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub stage_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<ThreadLimit>,
}

/// Diff comments to the issue's implementation. The implementation agent is
/// the run's own; an `agent_id` naming the ISSUE's agent is not on that
/// roster, so this verb does not take one.
#[derive(Debug, Deserialize, Serialize)]
pub struct RequestChangesParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<ReviewerMessageParams>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewing_context: Option<crate::thread::ViewingContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<ThreadLimit>,
}

/// Publish an issue's implementation: `commit`, `push`, `merge`,
/// `merge_push`. Documented as open.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueGitActionParams {
    #[serde(flatten)]
    pub issue: IssueRef,
    pub action: String,
    /// `prune` (the default), `keep`, or `release` — merge actions only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<ThreadLimit>,
}

/// The human has read one entry as it stands.
#[derive(Debug, Deserialize, Serialize)]
pub struct EntitySeenParams {
    pub entity_id: String,
    /// One bubble read through; the whole entry when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_from_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_through_sequence: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EntityMuteParams {
    pub entity_id: String,
    pub muted: bool,
}

/// A row cleared out of the inbox, named either by the entity behind it or —
/// for a checkout Build never cut — by what it is.
#[derive(Debug, Deserialize, Serialize)]
pub struct EntityDismissParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// The reviewer disagreeing with how one hunk was classified.
#[derive(Debug, Deserialize, Serialize)]
pub struct TriageOverrideParams {
    pub run_id: String,
    pub hunk_id: String,
    /// `surface` or `collapse`.
    pub direction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

// ---------------------------------------------------- issue/plan: results ---

/// Where an entity sits in the inbox, and whether the human has seen where it
/// got to.
#[derive(Debug, Deserialize, Serialize)]
pub struct AttentionView {
    pub resume_at: String,
    pub interacted: bool,
    pub seen: bool,
    pub anchor: String,
}

/// The run agent currently speaking for an issue.
#[derive(Debug, Deserialize, Serialize)]
pub struct ExecutionContext {
    pub entity_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub agent: AgentDigest,
}

/// One branch that implements, or implemented, this issue. `created_at` rides
/// along on the lineage listing and is absent on the current one.
#[derive(Debug, Deserialize, Serialize)]
pub struct ImplementationRef {
    pub implementation_id: String,
    pub run_id: String,
    pub state: String,
    pub branch: String,
    pub worktree_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<crate::run::RecoveryAttempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// What Done on this issue would gloss over.
#[derive(Debug, Deserialize, Serialize)]
pub struct FinishWarning {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FinishView {
    pub warnings: Vec<FinishWarning>,
}

/// One comment on a stage document — a post on the issue agent's own
/// conversation, anchored to the passage it is about.
#[derive(Debug, Deserialize, Serialize)]
pub struct CommentView {
    pub id: String,
    pub stage_id: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<crate::thread::DocAnchor>,
    pub body: String,
    /// `open` or `addressed`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_reply: Option<String>,
}

/// One stage of an issue: its document, its plan-side review sub-state, and —
/// on the listings that read a run — where its execution got to.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueStageRow {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub path: String,
    /// `planned` or `approved`.
    pub state: String,
    pub open_comments: u64,
    /// The same sub-state as `state`, under the name the stage board reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
    /// `pending`, `building`, `built`, `validating`, `complete`,
    /// `validation_failed`, `incomplete`, or `legacy_unpinned`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<crate::run::StagePublication>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<crate::run::ValidationReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidation_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<CommentView>>,
}

/// The whole issue: what it is for, where it got to, who is working on it,
/// and what has been built for it. Every `issue.*` mutation answers with it,
/// so a surface repaints from the mutation's own reply.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueView {
    pub issue_id: String,
    /// The same id under the name the `plan.*` alias speaks. Retired in 2.0.
    pub plan_id: String,
    pub goal: String,
    /// `created`, `drafting`, `plan_review`, `approved`, `blocked`, `failed`,
    /// `idle_unreported`, `interrupted`, `abandoned`.
    pub state: String,
    pub needs_attention: bool,
    /// The same fact as `needs_attention`, under the name the feed reads.
    pub unread: bool,
    pub unread_count: u64,
    /// The newest attention item's kind; absent exactly when the count is 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_reason: Option<String>,
    pub muted: bool,
    pub dismissed: bool,
    pub attention: AttentionView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub project: String,
    pub project_id: String,
    pub base_branch: String,
    pub plan_path: String,
    pub harness: String,
    pub provider: crate::models::AgentProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub thread: ThreadPayload,
    /// The rail's bubble strip, one entry per agent.
    pub agents: Vec<AgentDigest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_context: Option<ExecutionContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run_id: Option<String>,
    /// A branch is building this issue right now.
    pub implementation_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementing_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_implementation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_implementation: Option<ImplementationRef>,
    pub implementation_lineage: Vec<ImplementationRef>,
    pub implementation_intent: crate::plan::ImplementationIntent,
    pub implementation_activity: crate::plan::ImplementationActivity,
    pub implementation_complete: bool,
    pub can_archive: bool,
    pub finish: FinishView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    /// False when the store holds no docs: the client disables the doc reads
    /// instead of retrying reads that can never succeed.
    pub docs_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_changed_at: Option<String>,
    pub stages: Vec<IssueStageRow>,
}

/// `issue.list` — the same list twice, under both names, for the clients that
/// predate the rename.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueListResult {
    pub issues: Vec<IssueView>,
    pub plans: Vec<IssueView>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PlanListResult {
    pub plans: Vec<IssueView>,
}

/// The single-document issue's plan, read from the canonical store.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueDocResult {
    pub plan_path: String,
    pub contents: String,
}

/// The stage board of a multi-stage issue, with each stage's comments.
#[derive(Debug, Deserialize, Serialize)]
pub struct PlanStagesResult {
    pub issue_id: String,
    pub plan_id: String,
    pub stages: Vec<IssueStageRow>,
}

/// The stage board plus what the current implementation has done to it.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueStagesResult {
    pub issue_id: String,
    pub plan_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation_id: Option<String>,
    pub auto_advance: bool,
    pub stages: Vec<IssueStageRow>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StageDocResult {
    pub issue_id: String,
    pub plan_id: String,
    pub stage_id: String,
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CommentAddResult {
    pub comment: CommentView,
}

/// The verb did what it was asked and has nothing to report back.
#[derive(Debug, Deserialize, Serialize)]
pub struct Acknowledged {
    pub ok: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MuteResult {
    pub entity_id: String,
    pub muted: bool,
}

/// A dismissed row named by its entity.
#[derive(Debug, Deserialize, Serialize)]
pub struct EntityDismissed {
    pub entity_id: String,
    pub dismissed: bool,
}

/// A dismissed row with no entity behind it: a project's checkout, named by
/// what it is.
#[derive(Debug, Deserialize, Serialize)]
pub struct RowDismissed {
    pub project_id: String,
    /// Absent on a detached checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub dismissed: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DismissResult {
    /// Named first: only this variant carries `entity_id`.
    Entity(EntityDismissed),
    Row(RowDismissed),
}

/// The project-level rule one disagreement moved, and how many disagreements
/// now stand behind it.
#[derive(Debug, Deserialize, Serialize)]
pub struct TriageRule {
    pub pattern: String,
    pub direction: String,
    /// Absent when the reviewer said the same thing about the same hunk twice
    /// — one disagreement, so the count does not move.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

/// The triage pass as the review surface renders it.
#[derive(Debug, Deserialize, Serialize)]
pub struct TriageView {
    pub based_on: String,
    pub hunks: Vec<crate::run::TriageHunk>,
    pub overrides: Vec<crate::run::TriageOverride>,
    /// The diff moved under the pass.
    pub stale: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TriageOverrideResult {
    pub run_id: String,
    pub hunk_id: String,
    pub direction: String,
    pub path: String,
    pub rule: TriageRule,
    /// Absent on a run with no triage pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<TriageView>,
}

// --------------------------------------------------- issue/plan: handlers ---

fn issue_create(
    app: &mut AppState,
    params: IssueCreateParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.plan_create(&params.wire()))
}

fn issue_get(app: &mut AppState, params: IssueDetailParams) -> Result<Answer<IssueView>, ApiError> {
    answer(app.plan_get(&params.wire()))
}

fn issue_list(app: &mut AppState, _params: NoParams) -> Result<Answer<IssueListResult>, ApiError> {
    answer(Ok(app.issue_list()))
}

fn plan_list(app: &mut AppState, _params: NoParams) -> Result<Answer<PlanListResult>, ApiError> {
    answer(Ok(app.plan_list()))
}

fn issue_doc(
    app: &mut AppState,
    params: IssueRefParams,
) -> Result<Answer<IssueDocResult>, ApiError> {
    answer(app.plan_doc(&params.wire()))
}

fn issue_stages(
    app: &mut AppState,
    params: IssueRefParams,
) -> Result<Answer<IssueStagesResult>, ApiError> {
    answer(app.issue_stages(&params.wire()))
}

fn plan_stages(
    app: &mut AppState,
    params: IssueRefParams,
) -> Result<Answer<PlanStagesResult>, ApiError> {
    answer(app.plan_stages(&params.wire()))
}

fn issue_stage_doc(
    app: &mut AppState,
    params: StageRefParams,
) -> Result<Answer<StageDocResult>, ApiError> {
    answer(app.plan_stage_doc(&params.wire()))
}

fn issue_approve(
    app: &mut AppState,
    params: IssueMutationParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.plan_approve(&params.wire()))
}

fn issue_send_notes(
    app: &mut AppState,
    params: SendNotesParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.plan_send_notes(&params.wire()))
}

fn issue_stage_approve(
    app: &mut AppState,
    params: StageMutationParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.plan_stage_approve(&params.wire()))
}

fn issue_stage_revise(
    app: &mut AppState,
    params: StageMutationParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.plan_stage_send_notes(&params.wire()))
}

fn issue_message(
    app: &mut AppState,
    params: IssueMessageParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.plan_message(&params.wire()))
}

fn issue_abandon(
    app: &mut AppState,
    params: IssueMutationParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.plan_abandon(&params.wire()))
}

fn issue_comment_add(
    app: &mut AppState,
    params: CommentAddParams,
) -> Result<Answer<CommentAddResult>, ApiError> {
    answer(app.plan_comment_add(&params.wire()))
}

fn issue_comment_delete(
    app: &mut AppState,
    params: CommentDeleteParams,
) -> Result<Answer<Acknowledged>, ApiError> {
    answer(app.plan_comment_delete(&params.wire()))
}

fn issue_archive(
    app: &mut AppState,
    params: IssueMutationParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.plan_archive(&params.wire()))
}

fn issue_delete(
    app: &mut AppState,
    params: IssueRefParams,
) -> Result<Answer<Acknowledged>, ApiError> {
    answer(app.plan_delete(&params.wire()))
}

fn issue_implement_stage(
    app: &mut AppState,
    params: ImplementStageParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.issue_implement_stage(&params.wire()))
}

fn issue_implement_all(
    app: &mut AppState,
    params: ImplementAllParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.issue_implement_all(&params.wire()))
}

fn issue_set_auto_advance(
    app: &mut AppState,
    params: SetAutoAdvanceParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.issue_set_auto_advance(&params.wire()))
}

fn issue_stage_fix(
    app: &mut AppState,
    params: StageFixParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.issue_run_action(&params.wire(), "fix"))
}

fn issue_request_changes(
    app: &mut AppState,
    params: RequestChangesParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.issue_run_action(&params.wire(), "request_changes"))
}

fn issue_git_action(
    app: &mut AppState,
    params: IssueGitActionParams,
) -> Result<Answer<IssueView>, ApiError> {
    answer(app.issue_run_action(&params.wire(), "git_action"))
}

fn entity_seen(
    app: &mut AppState,
    params: EntitySeenParams,
) -> Result<Answer<Acknowledged>, ApiError> {
    answer(app.entity_seen(&params.wire()))
}

fn entity_mute(
    app: &mut AppState,
    params: EntityMuteParams,
) -> Result<Answer<MuteResult>, ApiError> {
    answer(app.entity_mute(&params.wire()))
}

fn entity_dismiss(
    app: &mut AppState,
    params: EntityDismissParams,
) -> Result<Answer<DismissResult>, ApiError> {
    answer(app.entity_dismiss(&params.wire()))
}

fn triage_override(
    app: &mut AppState,
    params: TriageOverrideParams,
) -> Result<Answer<TriageOverrideResult>, ApiError> {
    answer(app.triage_override(&params.wire()))
}

// ------------------------------------------------------ issue/plan: tests ---

#[cfg(test)]
mod issue_plan_tests {
    use super::*;
    use crate::api::v1::parse_params;

    fn round_trips(method: &str) {
        crate::api::v1::testing::fixture_round_trips(methods(), method);
    }

    /// Both spellings of an id reach the implementation, which still reads the
    /// deprecated one — the whole of what `alias_param` used to do by hand.
    #[test]
    fn an_issue_id_reaches_the_implementation_under_both_names() {
        let canonical: IssueRefParams =
            parse_params(&serde_json::json!({ "issue_id": "issue-7" })).unwrap();
        let deprecated: IssueRefParams =
            parse_params(&serde_json::json!({ "plan_id": "issue-7" })).unwrap();
        let expected = serde_json::json!({ "issue_id": "issue-7", "plan_id": "issue-7" });
        assert_eq!(canonical.wire(), expected);
        assert_eq!(deprecated.wire(), expected);
    }

    /// A limit nobody can read still says "this client can page", so the
    /// facade carries it through rather than refusing the frame — the
    /// implementation is where an unreadable limit becomes the default page.
    #[test]
    fn an_unreadable_thread_limit_is_carried_not_refused() {
        for spelling in [
            serde_json::json!(20),
            serde_json::json!("20"),
            serde_json::json!(20.0),
            serde_json::json!(true),
        ] {
            let params: IssueDetailParams = parse_params(&serde_json::json!({
                "issue_id": "issue-7",
                "thread_limit": spelling,
            }))
            .unwrap_or_else(|error| panic!("{spelling}: {}", error.message()));
            assert_eq!(params.wire()["thread_limit"], spelling);
        }
        let unspoken: IssueDetailParams = parse_params(&serde_json::json!({
            "issue_id": "issue-7",
            "thread_limit": serde_json::Value::Null,
        }))
        .unwrap();
        assert!(unspoken.wire().get("thread_limit").is_none());
    }

    macro_rules! fixture_tests {
        ($($name:ident => $method:literal),* $(,)?) => {
            $(
                #[test]
                fn $name() {
                    round_trips($method);
                }
            )*
        };
    }

    fixture_tests! {
        issue_create => "issue.create",
        plan_create => "plan.create",
        issue_get => "issue.get",
        plan_get => "plan.get",
        issue_list => "issue.list",
        plan_list => "plan.list",
        issue_doc => "issue.doc",
        plan_doc => "plan.doc",
        issue_stages => "issue.stages",
        plan_stages => "plan.stages",
        issue_stage_doc => "issue.stage_doc",
        plan_stage_doc => "plan.stage_doc",
        issue_approve => "issue.approve",
        plan_approve => "plan.approve",
        issue_send_notes => "issue.send_notes",
        plan_send_notes => "plan.send_notes",
        issue_stage_approve => "issue.stage_approve",
        plan_stage_approve => "plan.stage_approve",
        issue_stage_revise => "issue.stage_revise",
        plan_stage_send_notes => "plan.stage_send_notes",
        plan_message => "plan.message",
        plan_abandon => "plan.abandon",
        issue_comment_add => "issue.comment_add",
        plan_comment_add => "plan.comment_add",
        issue_comment_delete => "issue.comment_delete",
        plan_comment_delete => "plan.comment_delete",
        issue_archive => "issue.archive",
        plan_archive => "plan.archive",
        issue_delete => "issue.delete",
        plan_delete => "plan.delete",
        issue_implement_stage => "issue.implement_stage",
        issue_implement_all => "issue.implement_all",
        issue_set_auto_advance => "issue.set_auto_advance",
        issue_stage_fix => "issue.stage_fix",
        issue_request_changes => "issue.request_changes",
        issue_git_action => "issue.git_action",
        entity_seen => "entity.seen",
        entity_mute => "entity.mute",
        entity_dismiss => "entity.dismiss",
        triage_override => "triage.override",
    }
}

// ==== run/branch/worktree ==================================================

//
// The run lifecycle (`run.*`), the work item the feed and the URLs speak
// (`branch.get` / `branch.dispatch` / `branch.finish`), and the bare checkout
// verbs beside them (`worktree.create` / `worktree.finish`).
//
// Every mutation here answers with the run view the pre-facade implementation
// already built — [`RunView`] names its shape — except the ones that answer
// about a checkout instead ([`WorkspaceFinishResult`],
// [`CreatedWorktreeResult`], [`DispatchedAgentResult`])
// and the two that only acknowledge ([`RunAck`]). `run.create`, `run.abandon`,
// `run.delete`, `run.finish`, `branch.dispatch`, `branch.finish`,
// `worktree.create` and `worktree.finish` hand their git to the off-lock
// drain, so what the handler itself returns is the placeholder [`Answer`]
// documents and the drain fills in.
//
// The shared wire above is shared: [`ThreadWindowParams`],
// [`ReviewerMessageParams`], [`AttentionView`], [`FinishView`] and
// [`TriageView`] are the same shapes on both halves of the family.

// ---------------------------------------- run/branch/worktree: params ---

/// What an agent this call starts runs on. Absent everywhere means the
/// entity's own choice, never a silent default swap; naming any one of the
/// three counts as choosing.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct RunAgentChoiceParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// The run a verb acts on, and the conversation slice its answer carries.
#[derive(Debug, Deserialize, Serialize)]
pub struct RunViewParams {
    pub run_id: String,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

/// A run named and nothing else asked of it: the two verbs that answer an
/// acknowledgement rather than a view.
#[derive(Debug, Deserialize, Serialize)]
pub struct RunIdParams {
    pub run_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunCreateParams {
    /// The Issue to implement. `issue_id` is the spelling the rest of the v1
    /// surface uses and is read as an alias; what travels on to the
    /// implementation is `plan_id`, which is still the durable identity of an
    /// Issue and what the refusal for a missing one names.
    #[serde(alias = "issue_id")]
    pub plan_id: String,
    /// Implement into a checkout that already exists rather than one cut for
    /// the Issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    /// What the run's branch is cut from; the Issue's own base when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(flatten)]
    pub choice: RunAgentChoiceParams,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunRequestChangesParams {
    pub run_id: String,
    /// The anchored form: up to 100 comments, each with a non-empty body,
    /// each anchored (if at all) to a hunk of the diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<ReviewerMessageParams>>,
    /// The one-body form a client that anchors nothing still sends. Exactly
    /// one of `messages` and `comments` is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<String>,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

/// A stage of a run named: `run.stage_send_notes`, and the base the two stage
/// verbs that ask for more are built on.
#[derive(Debug, Deserialize, Serialize)]
pub struct RunStageParams {
    pub run_id: String,
    pub stage_id: String,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunStageDispatchParams {
    pub run_id: String,
    pub stage_id: String,
    #[serde(flatten)]
    pub choice: RunAgentChoiceParams,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunStageFixParams {
    pub run_id: String,
    pub stage_id: String,
    /// What to fix, in the reviewer's words; absent when the failed
    /// validation speaks for itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunAutoAdvanceParams {
    pub run_id: String,
    /// Arm ("run all") or disarm. Required: there is no default answer to
    /// "should this keep going by itself".
    pub enabled: bool,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunGitActionParams {
    pub run_id: String,
    /// `commit`, `push`, `merge`, or `merge_push`.
    pub action: String,
    /// What happens to the checkout after a merge lands: `prune` (the
    /// default), `keep`, or `release`. Refused on a non-merge action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<String>,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunMessageParams {
    pub run_id: String,
    pub message: String,
    /// What the human was looking at when they said it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewing_context: Option<crate::thread::ViewingContext>,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunAdoptParams {
    pub project_id: String,
    /// The external checkout to adopt. Required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    /// Retired: the project's own checkout is what workspaces are cut from,
    /// never a place to work. Carried so a client that still sends it is
    /// refused in words rather than told a param is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<bool>,
    #[serde(flatten)]
    pub choice: RunAgentChoiceParams,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunFinishParams {
    pub run_id: String,
    /// How the checkout is retired: `delete`, `cleanup`, `push`, or `merge`.
    ///
    /// Optional since the finish became the workspace's: the workspace decides
    /// what retiring it means, and callers that still send an action are
    /// accepted unchanged rather than refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BranchGetParams {
    pub project_id: String,
    pub branch: String,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BranchDispatchParams {
    pub project_id: String,
    /// What the agent is being asked to do. Never empty — an empty
    /// instruction is nothing to dispatch.
    pub instruction: String,
    /// Where the work goes; with none, the instruction names the branch this
    /// call cuts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(flatten)]
    pub choice: RunAgentChoiceParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BranchFinishParams {
    pub project_id: String,
    pub branch: String,
    /// `delete` (what Done means, and the default), `cleanup`, `push`, or
    /// `merge`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Leave the issue this branch implemented alone, whichever way the
    /// branch ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlink: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WorktreeCreateParams {
    pub project_id: String,
    /// A branch that already exists, here or on a remote: Build borrows it a
    /// directory and cuts nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Words to cut a new branch after. Exactly one of `branch` and `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WorktreeFinishParams {
    pub project_id: String,
    pub worktree_id: String,
    /// `delete`, `cleanup`, `push`, or `merge`. Optional for the same reason
    /// [`RunFinishParams::action`] is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

// --------------------------------------- run/branch/worktree: results ---

/// One stage's run-side execution progress. A record exists only once the
/// stage has been dispatched.
#[derive(Debug, Deserialize, Serialize)]
pub struct RunStageView {
    pub id: String,
    /// `building`, `built`, `validating`, `validated_passed`, or
    /// `validated_failed`.
    pub state: String,
    /// HEAD when the stage was first dispatched — the base of "the diff this
    /// stage produced".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_sha: Option<String>,
    /// The immutable successful boundary `run.stage_diff` reads against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_sha: Option<String>,
    /// `local`, `pushed`, `merged`, or `legacy_unknown`.
    pub publication: String,
    /// Why the stage's evidence stopped being readable — worktree loss, or a
    /// recovery that moved the branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidation_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<crate::run::ValidationReport>,
}

/// A run as every run verb answers it: identity and issue link, lifecycle
/// state, the conversation and its agents, the checkout, per-stage progress,
/// and what the inbox needs to place it.
#[derive(Debug, Deserialize, Serialize)]
pub struct RunView {
    pub run_id: String,
    /// The run id again, under the name the Issue surface reads it by.
    pub implementation_id: String,
    /// The Issue this run implements, when it implements one. `plan_id` is
    /// the same id under the deprecated spelling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    pub goal: String,
    /// `created`, `building`, `stage_gate`, `review`, `blocked`, `failed`,
    /// `idle_unreported`, `interrupted`, `merged`, `abandoned`, `archived`.
    pub state: String,
    /// An attention-class item landed past the human's read cursor.
    pub needs_attention: bool,
    /// The same fact, under the name the SPA already reads.
    pub unread: bool,
    pub unread_count: u64,
    /// What the newest attention item was; absent exactly when the count is
    /// zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_reason: Option<String>,
    /// Told to stop asking: the badge is zeroed while the cursor keeps the
    /// truth.
    pub muted: bool,
    /// Cleared out of the inbox until the conversation asks again.
    pub dismissed: bool,
    pub attention: AttentionView,
    pub branch: String,
    pub base_branch: String,
    /// The materialization commit the review diff is anchored on; absent for
    /// an adopted or migrated run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    pub worktree_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub project: String,
    pub project_id: String,
    pub harness: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub thread: ThreadPayload,
    pub agents: Vec<AgentDigest>,
    /// The last triage pass over this run's diff; absent on a run that has
    /// had none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<TriageView>,
    pub triage_enabled: bool,
    /// "Run all" is armed.
    pub auto_advance: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_stage_id: Option<String>,
    /// Minted around a checkout the human already had.
    pub adopted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<crate::run::RecoveryAttempt>,
    /// Whether `run.finish` would be accepted, so the control is never
    /// offered where it would be refused.
    pub can_finish: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_changed_at: Option<String>,
    pub stages: Vec<RunStageView>,
}

/// A verb that only says it happened. `run.delete` on a planned
/// implementation keeps the record — an Issue's lineage outlives its card —
/// and says so.
#[derive(Debug, Deserialize, Serialize)]
pub struct RunAck {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_as_issue_lineage: Option<bool>,
}

/// What was sitting in a checkout's tree that no commit held.
#[derive(Debug, Deserialize, Serialize)]
pub struct UncommittedStat {
    pub files_changed: u64,
    pub insertions: u64,
    pub deletions: u64,
}

/// What a checkout carries, as the inbox counts it.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkItemStatView {
    pub files_changed: u64,
    pub insertions: u64,
    pub deletions: u64,
    pub uncommitted: UncommittedStat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// The one ref both counts are measured against: the tracking branch, or
    /// the project's base branch when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison_ref: Option<String>,
}

/// How long the agent on this row has been working.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkingTimeView {
    pub since: String,
    /// Absent when the timestamp could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
}

/// One inbox row for a branch, with the run behind it when Build owns one:
/// the row `board.list` renders, plus `run`.
#[derive(Debug, Deserialize, Serialize)]
pub struct BranchWorkItem {
    /// Always `branch` here.
    pub kind: String,
    pub project_id: String,
    pub project: String,
    /// Absent only for a checkout git could name no branch for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub title: String,
    /// The run's state, or `idle` for a checkout Build owns no run in.
    pub state: String,
    pub unread: bool,
    pub unread_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_reason: Option<String>,
    pub working: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_time: Option<WorkingTimeView>,
    pub agents: Vec<AgentDigest>,
    pub stat: WorkItemStatView,
    /// The sort key the inbox orders on; absent on a row with no record
    /// behind it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_at: Option<String>,
    /// Where this row sits in the inbox; absent for a checkout with neither a
    /// commit nor a record to date it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// When somebody last spoke on this work item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    pub can_finish: bool,
    /// What Done would cost. Warnings the client confirms through, never a
    /// refusal.
    pub finish: FinishView,
    pub muted: bool,
    pub dismissed: bool,
    pub worktree_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<String>,
    /// The run behind the branch; absent for a bare checkout, which has no
    /// agent to name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Box<RunView>>,
}

/// One repository a finish tried to push, and whether it went.
#[derive(Debug, Deserialize, Serialize)]
pub struct FinishedRepository {
    pub directory_id: String,
    pub pushed: bool,
    /// Why it did not push. Absent when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What Done answers, for every one of its spellings.
///
/// `run.finish`, `branch.finish` and `worktree.finish` all resolve to the
/// workspace behind the id they were given and run the same finish
/// (`AppState::workspace_finish_legacy`), so all three answer this: whether
/// the whole workspace came to rest, and what became of each repository in
/// it, and that it is gone. The old per-checkout archive record went with the
/// per-checkout finish that produced it.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceFinishResult {
    pub complete: bool,
    pub repositories: Vec<FinishedRepository>,
    /// Always `true`: Done removes the workspace it finished. The record of
    /// what was finished stays in the archive; the files and the live record
    /// do not.
    #[serde(default)]
    pub deleted: bool,
}

/// Somebody else's adoption of this checkout is already in flight. The asker
/// is handed no run id — the run that will carry the checkout is not in the
/// map until that adoption's epilogue lands — and asks again.
#[derive(Debug, Deserialize, Serialize)]
pub struct AdoptionInFlight {
    /// Always `true`.
    pub adopting: bool,
}

/// What `run.adopt` answers: the run that owns the checkout, or the latch
/// saying another caller is minting it right now.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RunAdoptResult {
    /// Named first: only this variant carries `adopting`.
    InFlight(AdoptionInFlight),
    Adopted(Box<RunView>),
}

/// The agent one dispatch put on a branch, and the branch it is working.
#[derive(Debug, Deserialize, Serialize)]
pub struct DispatchedAgentResult {
    pub project_id: String,
    pub branch: String,
    pub run_id: String,
    pub agent_id: String,
}

/// A bare worktree, cut.
#[derive(Debug, Deserialize, Serialize)]
pub struct CreatedWorktreeResult {
    pub project_id: String,
    pub worktree_id: String,
    /// The id the placeholder row stood under while the git ran.
    pub pending_worktree_id: String,
    /// A new branch was cut for it, rather than an existing one borrowed.
    pub branch_was_cut: bool,
    pub branch: String,
    pub name: String,
    pub path: String,
    /// `worktree` or `cow`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
    /// Why the isolation the project asked for could not be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation_note: Option<String>,
}

// ------------------------------------ run/branch/worktree: refinement ---

/// A refusal the run lifecycle raises that the generic classifier cannot
/// name: the state machine said no (conflict), something else holds the thing
/// (busy), or a word in the request is not one this bridge knows
/// (invalid_params).
///
/// The message is never rewritten — only the code beside it is decided.
fn refine(error: ApiError) -> ApiError {
    let message = error.message().to_string();
    if let Some(current) = bound_conversation_id(&message) {
        return ApiError::conflict(message, Some(serde_json::json!({ "current": current })));
    }
    if BUSY.iter().any(|marker| message.contains(marker)) {
        return ApiError::busy(message);
    }
    if CONFLICT.iter().any(|marker| message.contains(marker)) {
        return ApiError::conflict(message, None);
    }
    if NOT_FOUND.iter().any(|marker| message.contains(marker)) {
        return ApiError::not_found(message);
    }
    if INVALID.iter().any(|marker| message.contains(marker)) {
        return ApiError::invalid_params(message);
    }
    error
}

/// Something else is already doing this to the same thing, and the answer
/// after it finishes may be different. The only retryable class there is.
const BUSY: [&str; 2] = ["wait for that to finish", "wait for that to complete"];

/// The request was legible and the state said no.
const CONFLICT: [&str; 11] = [
    // Done, refused because the work is still only in the workspace, or
    // because the checkout is not Build's to remove.
    "workspace.finish is not available yet",
    "adopted checkouts are not Build's to remove",
    "Cannot delete a workspace",
    "illegal run transition",
    "only terminal runs",
    "only adopted runs can be released",
    "cannot set auto_advance on a terminal run",
    "cannot be merged",
    "cannot be finished or archived",
    "already started with action",
    "there are no plan docs to revise",
];

/// The thing named does not exist here, though the message does not start
/// with the word the generic classifier looks for.
const NOT_FOUND: [&str; 2] = [
    "no checkout of this project is on branch",
    // Done, given an id that resolves to no workspace.
    "no matching workspace",
];

/// A word in the request is not one this bridge knows. `unknown <thing>`
/// otherwise reads as a missing entity, which these are not.
const INVALID: [&str; 5] = [
    "unknown git action:",
    "unknown agent provider:",
    "is not a branch name",
    "takes exactly one of",
    "needs at least one letter or number",
];

/// The conversation a stale `conversation_id` should have named: what the
/// agent is bound to now, so the retry has somewhere to go.
fn bound_conversation_id(message: &str) -> Option<String> {
    if !message.starts_with("stale conversation_id ") {
        return None;
    }
    Some(message.rsplit(" is bound to ").next()?.to_string())
}

// -------------------------------------- run/branch/worktree: handlers ---

fn run_create(app: &mut AppState, params: RunCreateParams) -> Result<Answer<RunView>, ApiError> {
    answer(app.run_create(&params.wire())).map_err(refine)
}

fn run_get(app: &mut AppState, params: RunViewParams) -> Result<Answer<RunView>, ApiError> {
    answer(app.run_get(&params.wire())).map_err(refine)
}

fn run_request_changes(
    app: &mut AppState,
    params: RunRequestChangesParams,
) -> Result<Answer<RunView>, ApiError> {
    answer(app.run_request_changes(&params.wire())).map_err(refine)
}

fn run_stage_dispatch(
    app: &mut AppState,
    params: RunStageDispatchParams,
) -> Result<Answer<RunView>, ApiError> {
    answer(app.run_stage_dispatch(&params.wire())).map_err(refine)
}

fn run_stage_fix(
    app: &mut AppState,
    params: RunStageFixParams,
) -> Result<Answer<RunView>, ApiError> {
    answer(app.run_stage_fix(&params.wire())).map_err(refine)
}

fn run_stage_send_notes(
    app: &mut AppState,
    params: RunStageParams,
) -> Result<Answer<RunView>, ApiError> {
    answer(app.run_stage_send_notes(&params.wire())).map_err(refine)
}

fn run_set_auto_advance(
    app: &mut AppState,
    params: RunAutoAdvanceParams,
) -> Result<Answer<RunView>, ApiError> {
    answer(app.run_set_auto_advance(&params.wire())).map_err(refine)
}

fn run_git_action(
    app: &mut AppState,
    params: RunGitActionParams,
) -> Result<Answer<RunView>, ApiError> {
    answer(app.run_git_action(&params.wire())).map_err(refine)
}

fn run_message(app: &mut AppState, params: RunMessageParams) -> Result<Answer<RunView>, ApiError> {
    answer(app.run_message(&params.wire())).map_err(refine)
}

fn run_abandon(app: &mut AppState, params: RunViewParams) -> Result<Answer<RunView>, ApiError> {
    answer(app.run_abandon(&params.wire())).map_err(refine)
}

fn run_delete(app: &mut AppState, params: RunIdParams) -> Result<Answer<RunAck>, ApiError> {
    answer(app.run_delete(&params.wire())).map_err(refine)
}

fn run_adopt(
    app: &mut AppState,
    params: RunAdoptParams,
) -> Result<Answer<RunAdoptResult>, ApiError> {
    answer(app.run_adopt(&params.wire())).map_err(refine)
}

fn run_release(app: &mut AppState, params: RunIdParams) -> Result<Answer<RunAck>, ApiError> {
    answer(app.run_release(&params.wire())).map_err(refine)
}

fn run_finish(
    app: &mut AppState,
    params: RunFinishParams,
) -> Result<Answer<WorkspaceFinishResult>, ApiError> {
    answer(
        app.workspace_finish_legacy(&params.wire())
            .map(deferral_placeholder),
    )
    .map_err(refine)
}

fn branch_get(
    app: &mut AppState,
    params: BranchGetParams,
) -> Result<Answer<BranchWorkItem>, ApiError> {
    answer(app.branch_get(&params.wire())).map_err(refine)
}

fn branch_dispatch(
    app: &mut AppState,
    params: BranchDispatchParams,
) -> Result<Answer<DispatchedAgentResult>, ApiError> {
    answer(app.branch_dispatch(&params.wire())).map_err(refine)
}

fn branch_finish(
    app: &mut AppState,
    params: BranchFinishParams,
) -> Result<Answer<WorkspaceFinishResult>, ApiError> {
    answer(
        app.workspace_finish_legacy(&params.wire())
            .map(deferral_placeholder),
    )
    .map_err(refine)
}

fn worktree_create(
    app: &mut AppState,
    params: WorktreeCreateParams,
) -> Result<Answer<CreatedWorktreeResult>, ApiError> {
    answer(app.worktree_create(&params.wire())).map_err(refine)
}

fn worktree_finish(
    app: &mut AppState,
    params: WorktreeFinishParams,
) -> Result<Answer<WorkspaceFinishResult>, ApiError> {
    answer(
        app.workspace_finish_legacy(&params.wire())
            .map(deferral_placeholder),
    )
    .map_err(refine)
}

// ----------------------------------------- run/branch/worktree: tests ---

#[cfg(test)]
mod run_branch_worktree_tests {
    use super::*;
    use crate::api::v1::parse_params;

    fn round_trips(method: &str) {
        crate::api::v1::testing::fixture_round_trips(methods(), method);
    }

    macro_rules! run_fixture_tests {
        ($($name:ident => $method:literal),* $(,)?) => {
            $(
                #[test]
                fn $name() {
                    round_trips($method);
                }
            )*
        };
    }

    run_fixture_tests! {
        run_create => "run.create",
        run_get => "run.get",
        run_request_changes => "run.request_changes",
        run_stage_dispatch => "run.stage_dispatch",
        run_stage_fix => "run.stage_fix",
        run_stage_send_notes => "run.stage_send_notes",
        run_set_auto_advance => "run.set_auto_advance",
        run_git_action => "run.git_action",
        run_message => "run.message",
        run_abandon => "run.abandon",
        run_delete => "run.delete",
        run_adopt => "run.adopt",
        run_release => "run.release",
        run_finish => "run.finish",
        branch_get => "branch.get",
        branch_dispatch => "branch.dispatch",
        branch_finish => "branch.finish",
        worktree_create => "worktree.create",
        worktree_finish => "worktree.finish",
    }

    /// An Issue can be named either way, and what reaches the implementation
    /// is the spelling it still reads.
    #[test]
    fn run_create_takes_an_issue_under_both_names() {
        let canonical: RunCreateParams =
            parse_params(&serde_json::json!({ "issue_id": "issue-7" })).unwrap();
        let deprecated: RunCreateParams =
            parse_params(&serde_json::json!({ "plan_id": "issue-7" })).unwrap();
        assert_eq!(canonical.wire()["plan_id"], "issue-7");
        assert_eq!(deprecated.wire()["plan_id"], "issue-7");
        let missing = parse_params::<RunCreateParams>(&serde_json::json!({ "goal": "quick" }))
            .expect_err("a run implements an issue or nothing");
        assert_eq!(missing.message(), "missing required param: plan_id");
        assert_eq!(missing.code(), "invalid_params");
    }

    #[test]
    fn a_stale_conversation_id_is_a_conflict_naming_the_binding_to_retry_with() {
        let refused = refine(ApiError::internal(
            "stale conversation_id conv-old; agent agent-3 is bound to conv-7",
        ));
        assert_eq!(refused.code(), "conflict");
        assert_eq!(
            refused.details(),
            Some(&serde_json::json!({ "current": "conv-7" }))
        );
        assert!(!refused.retryable());
    }

    #[test]
    fn a_claim_somebody_else_holds_is_the_one_retryable_refusal() {
        let refused = refine(ApiError::internal(
            "worktree wt-2 is already finishing — wait for that to complete",
        ));
        assert_eq!(refused.code(), "busy");
        assert!(refused.retryable());
    }

    #[test]
    fn the_state_machine_saying_no_is_a_conflict_and_a_missing_run_is_not() {
        let illegal = refine(ApiError::classify(
            "illegal run transition: Abandon is not valid from Merged".to_string(),
        ));
        assert_eq!(illegal.code(), "conflict");
        let missing = refine(ApiError::classify("unknown run_id".to_string()));
        assert_eq!(missing.code(), "not_found");
        let unnamed = refine(ApiError::classify("unknown git action: rebase".to_string()));
        assert_eq!(unnamed.code(), "invalid_params");
    }

    /// A branch nobody has a checkout on is a missing thing, not a broken
    /// bridge: the message says so and the code has to agree.
    #[test]
    fn a_branch_with_no_checkout_is_not_found() {
        let refused = refine(ApiError::classify(
            "branch.get: no checkout of this project is on branch build/gone (the scan has \
             settled)"
                .to_string(),
        ));
        assert_eq!(refused.code(), "not_found");
    }
}
