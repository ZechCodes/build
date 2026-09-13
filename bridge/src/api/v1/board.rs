//! The board family: the feed and the archive (`board.list`, `archive.list`,
//! `archived.list`), the project surface (`project.*` less the diff reads the
//! git family owns), the capture surface (`capture.*`), and what the Account
//! page reads and writes (`settings.*`, `models.list`).
//!
//! Same shape as `git.rs`: the implementations under `app/` are untouched —
//! each handler resolves its typed params, hands them to the implementation
//! that already exists, and names the shape that implementation answers in.
//! `project.list` and the five project mutations defer their git to the
//! off-lock drain, so what those handlers return is the placeholder
//! [`Answer`] documents; everything else here answers inline.
//!
//! Rows that belong to another family's entity — an issue view, a run view, a
//! feed item — are carried here as [`IssueRow`], [`RunRow`] and
//! [`FeedItemRow`]: the keys the board itself is read by are named and typed,
//! and the rest of the entity's shape rides in `rest` rather than being
//! restated (and left to drift) in a second place. `issue.get` and `run.get`
//! are where those shapes are stated whole.

use super::{answer, Answer, Handler, NoParams, WireParams};
use crate::api::ApiError;
use crate::app::AppState;
use crate::capture::Capture;
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The verbs this family serves.
pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!("board.list", board_list, NoParams, BoardListResult),
        v1_method!(
            "archive.list",
            archive_list,
            ProjectParams,
            ArchiveListResult
        ),
        v1_method!("archived.list", archived_list, NoParams, ArchivedListResult),
        v1_method!("project.list", project_list, NoParams, ProjectListResult),
        v1_method!("project.add", project_add, ProjectAddParams, ProjectRow),
        v1_method!(
            "project.create",
            project_create,
            ProjectCreateParams,
            ProjectRow
        ),
        v1_method!(
            "project.clone",
            project_clone,
            ProjectCloneParams,
            ProjectRow
        ),
        v1_method!(
            "project.init_git",
            project_init_git,
            ProjectParams,
            ProjectRow
        ),
        v1_method!(
            "project.set_remote",
            project_set_remote,
            ProjectSetRemoteParams,
            ProjectRow
        ),
        v1_method!(
            "project.set_isolation",
            project_set_isolation,
            ProjectSetIsolationParams,
            ProjectRow
        ),
        v1_method!(
            "capture.create",
            capture_create,
            CaptureCreateParams,
            Capture
        ),
        v1_method!("capture.list", capture_list, NoParams, CaptureListResult),
        v1_method!("capture.get", capture_get, CaptureParams, Capture),
        v1_method!(
            "capture.answer",
            capture_answer,
            CaptureAnswerParams,
            Capture
        ),
        v1_method!(
            "capture.reroute",
            capture_reroute,
            CaptureRerouteParams,
            Capture
        ),
        v1_method!(
            "capture.cancel",
            capture_cancel,
            CaptureParams,
            CaptureCancelled
        ),
        v1_method!("settings.get", settings_get, NoParams, SettingsResult),
        v1_method!(
            "settings.set",
            settings_set,
            SettingsSetParams,
            SettingsResult
        ),
        v1_method!("models.list", models_list, NoParams, ModelsListResult),
    ]
}

// ---------------------------------------------------------------- params ---

/// A verb that names one project and nothing else: `archive.list`,
/// `project.init_git`.
#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectParams {
    pub project_id: String,
}

/// One directory a multi-source project is opened over: a host path already
/// on this device, or a remote to clone into the project's own folder —
/// exactly one of the two.
#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectSourceParams {
    /// Absolute or `~`-relative. Mutually exclusive with `remote`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A clone url. Mutually exclusive with `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// The directory's own last segment (or the url's repo name) when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The source's own checked-out branch when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
}

/// Register a project over what is already on the device.
///
/// Two forms. `path` alone is the original one-repository project. `sources`
/// is the multi-source form: one project over several directories, which the
/// implementation takes instead of `path` when it is present — which is why
/// `path` is optional HERE and required THERE. Sending neither is refused by
/// the implementation with the `missing required param: path` it always sent.
#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectAddParams {
    /// A host path, absolute or `~`-relative; it must already be a git repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The repository's own checked-out branch when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    /// The multi-source form. Takes precedence over `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<ProjectSourceParams>>,
    /// Names the multi-source project; the first source's name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Make a project's repository, or open one over several directories the way
/// [`ProjectAddParams`] describes. `sources`, when present, is what is used
/// and the rest of these are ignored.
#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectCreateParams {
    pub name: String,
    /// The multi-source form. Takes precedence over `parent`/`remote`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<ProjectSourceParams>>,
    /// `main` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    /// The directory to make the repository in; the projects folder when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Wired as `origin` at creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectCloneParams {
    pub url: String,
    /// The folder to clone into, under the projects folder; derived from the
    /// URL's last segment when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The clone's own checked-out branch when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectSetRemoteParams {
    pub project_id: String,
    /// An empty url clears `origin`.
    pub url: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectSetIsolationParams {
    pub project_id: String,
    /// `worktree` or `cow`, and `null` to go back to inheriting the account's
    /// choice. Required — naming no isolation at all is a missing param while
    /// naming `null` is the clear, which is why it reads through
    /// [`required_nullable`] (serde would otherwise let a missing field pass
    /// as `None`) and is not skipped when it serialises.
    #[serde(deserialize_with = "required_nullable")]
    pub isolation: Option<String>,
}

/// A field that must be there and may be `null`. Serde reads a missing
/// `Option` field as `None`, which would turn "you forgot to say" into "clear
/// it"; a `deserialize_with` field is required, so the two stay distinct.
fn required_nullable<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::deserialize(deserializer)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CaptureCreateParams {
    /// What the user said. Kept before anything routes it.
    pub text: String,
}

/// A verb that names one capture and nothing else: `capture.get`,
/// `capture.cancel`.
#[derive(Debug, Deserialize, Serialize)]
pub struct CaptureParams {
    pub capture_id: String,
}

/// The user's answer to the router's question: words they typed, or one of
/// the options it offered — by id, or by the position it was offered in.
#[derive(Debug, Deserialize, Serialize)]
pub struct CaptureAnswerParams {
    pub capture_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    /// Counting from the first option offered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_index: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CaptureRerouteParams {
    pub capture_id: String,
    /// Where the user is sending it. With no project the router is asked
    /// again, which is what the one-tap retry on a failed route is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// `issue` (the default) or `branch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The branch a branch reroute continues; named after what was said when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// One `settings.set`: field-wise, so a client sets what it names and leaves
/// the rest of the account exactly as it was. Naming nothing at all is
/// refused rather than answered as a no-op.
///
/// Every field is a [`Named`]: absence and `null` are different requests
/// here, and only the account itself gets to say which of the two it will
/// take, so a `null` is carried to it rather than quietly dropped on the way.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct SettingsSetParams {
    /// Where clones land. The folder is made, or the set is refused.
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub projects_dir: Named<String>,
    /// `claude_adk`, `claude`, `codex`, `codex_app_server` or `pi`.
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub default_harness: Named<String>,
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub agent_modes: Named<AgentModesPatch>,
    /// Deprecated alias: names the concrete harness the Claude family opens
    /// on. Read after `default_harness`, so the newer word wins.
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub claude_mode: Named<String>,
    /// Deprecated alias, as `claude_mode` is.
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub codex_mode: Named<String>,
    /// `worktree` or `cow`.
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub isolation: Named<String>,
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub triage_enabled: Named<bool>,
}

/// A field a client either did not mention (`None`) or named — with a value,
/// or with `null`. Serde reads both a missing field and a `null` one as
/// `None`, which would make "leave this alone" and "here is a null" the same
/// request; read through [`named`] they stay distinct, and the `null` reaches
/// the implementation that refuses it.
pub type Named<T> = Option<Option<T>>;

/// The read [`Named`] needs: a `deserialize_with` field is required unless it
/// carries a default, so the default is what says "not mentioned" and this is
/// only ever called on a field that WAS mentioned.
fn named<'de, D, T>(deserializer: D) -> Result<Named<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

/// The families a `settings.set` may re-open: each `headless` or `tui`, and
/// the ones left unnamed keep what the account holds.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct AgentModesPatch {
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub claude: Named<String>,
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    pub codex: Named<String>,
}

// --------------------------------------------------------------- results ---

/// What this volume can make, and the sentence a control shows when it
/// cannot. Always both keys: `reason` is `null` when nothing is locked.
///
/// One isolation is named, and it is `rift`: the copy-on-write checkout the
/// `cow` key stood for was replaced by Rift, so the key was too.
#[derive(Debug, Deserialize, Serialize)]
pub struct IsolationAvailabilityView {
    pub rift: bool,
    pub reason: Option<String>,
}

/// A project as every project verb answers with it: what it is, and the whole
/// isolation picture a control paints from.
#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectRow {
    pub project_id: String,
    pub name: String,
    pub path: String,
    pub base_branch: String,
    pub is_git: bool,
    /// `origin`, or `null` where there is none.
    pub remote: Option<String>,
    /// What this project chose; `null` while it inherits the account's.
    pub isolation: Option<String>,
    pub isolation_default: String,
    /// What this project's next checkout will actually be.
    pub isolation_effective: String,
    pub isolation_available: IsolationAvailabilityView,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectListResult {
    pub projects: Vec<ProjectRow>,
}

/// The board's own thinner project row: enough to name and group by, without
/// the isolation picture `project.list` is read for.
#[derive(Debug, Deserialize, Serialize)]
pub struct BoardProjectRow {
    pub project_id: String,
    pub name: String,
    pub path: String,
    pub base_branch: String,
    pub is_git: bool,
}

/// The keys of an entity view that the board is read by, over the rest of the
/// shape `issue.get` / `run.get` / `branch.get` state whole.
pub type OtherKeys = BTreeMap<String, serde_json::Value>;

/// One issue on the board (`board.list`'s `issues` and `plans`, and
/// `archive.list`'s `plans`).
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueRow {
    pub issue_id: String,
    /// The same id under the name the pre-redesign SPA reads.
    pub plan_id: String,
    pub goal: String,
    pub state: String,
    pub project_id: String,
    pub project: String,
    pub muted: bool,
    pub dismissed: bool,
    pub unread: bool,
    /// The same fact as `unread`, under the name the SPA already reads.
    pub needs_attention: bool,
    #[serde(flatten)]
    pub rest: OtherKeys,
}

/// One run on the board.
#[derive(Debug, Deserialize, Serialize)]
pub struct RunRow {
    pub run_id: String,
    pub branch: String,
    pub state: String,
    pub project_id: String,
    pub project: String,
    pub worktree_path: String,
    /// A run standing on the project's own checkout rather than a worktree.
    pub primary: bool,
    pub muted: bool,
    pub dismissed: bool,
    pub unread: bool,
    #[serde(flatten)]
    pub rest: OtherKeys,
}

/// One row of the feed: a branch, an issue, or a capture still deciding where
/// it goes.
#[derive(Debug, Deserialize, Serialize)]
pub struct FeedItemRow {
    /// `branch`, `issue` or `capture`.
    pub kind: String,
    pub title: Option<String>,
    pub state: Option<String>,
    pub project_id: String,
    pub project: String,
    pub primary: bool,
    pub muted: bool,
    pub dismissed: bool,
    pub unread: bool,
    /// Where the row sits in the list: the moment the work was first said.
    pub anchor: Option<String>,
    pub resume_at: Option<String>,
    pub last_activity: Option<String>,
    #[serde(flatten)]
    pub rest: OtherKeys,
}

/// One external checkout the rail's scan found.
#[derive(Debug, Deserialize, Serialize)]
pub struct ExternalWorktreeRow {
    pub worktree_id: String,
    pub project_id: String,
    pub project: String,
    pub path: String,
    pub base_branch: String,
    pub isolation: String,
    /// `null` on a detached checkout.
    pub branch: Option<String>,
    pub adoptable: bool,
    pub agent_working: bool,
    pub can_finish: bool,
    #[serde(flatten)]
    pub rest: OtherKeys,
}

/// A lifecycle verb whose git is running right now, under the id the row it
/// is making will settle as.
#[derive(Debug, Deserialize, Serialize)]
pub struct PendingRow {
    pub entity_id: String,
    pub project_id: String,
    pub project: String,
    pub title: String,
    pub branch: Option<String>,
    pub state: String,
    pub checkout_id: Option<String>,
    pub primary: bool,
    /// The issue this checkout is being cut for, where there is one.
    pub implements: Option<String>,
    /// How the checkout being made is isolated; `null` where the verb makes
    /// none.
    pub isolation: Option<String>,
    /// How long this row has stood. Older than a scan interval reads as stuck
    /// rather than as work in flight.
    pub pending_seconds: u64,
}

/// One project's primary checkout as the last walk left it: what is sitting
/// in it, and where its branch stands against the remote.
#[derive(Debug, Deserialize, Serialize)]
pub struct PrimaryChangesRow {
    pub project_id: String,
    pub branch: String,
    pub upstream: Option<String>,
    pub comparison_ref: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub head_sha: Option<String>,
    pub head_committed_at: Option<String>,
    pub files_changed: u64,
    pub insertions: u64,
    pub deletions: u64,
    /// The run that owns this checkout, where one has adopted it.
    pub run_id: Option<String>,
}

/// The feed, and every ride-along a client would otherwise ask for one call
/// later.
#[derive(Debug, Deserialize, Serialize)]
pub struct BoardListResult {
    pub items: Vec<FeedItemRow>,
    pub projects: Vec<BoardProjectRow>,
    pub runs: Vec<RunRow>,
    pub external_worktrees: Vec<ExternalWorktreeRow>,
    pub pending: Vec<PendingRow>,
    pub primary_changes: Vec<PrimaryChangesRow>,
    /// The rail has not finished looking: an empty list under this flag is a
    /// board still working, not a project with no checkouts.
    pub scanning: bool,
    /// One entry per workspace, whatever project it belongs to.
    pub workspace_summaries: Vec<WorkspaceSummaryRow>,
}

/// What a workspace has done, across every git directory in it. `null` while
/// the off-lock walk has not answered yet, and for a workspace holding no
/// repository at all.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceSummaryRow {
    pub workspace_id: String,
    pub work_summary: Option<WorkSummaryView>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WorkSummaryView {
    pub pushes: u64,
    pub additions: u64,
    pub deletions: u64,
}

/// The line census an archived checkout was carrying when it went.
#[derive(Debug, Deserialize, Serialize)]
pub struct UncommittedStat {
    pub files_changed: u64,
    pub insertions: u64,
    pub deletions: u64,
}

/// One archived checkout of a project, as the record kept of it reads.
#[derive(Debug, Deserialize, Serialize)]
pub struct ArchivedWorktreeRow {
    pub worktree_id: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub head_sha: String,
    pub upstream: Option<String>,
    pub unpushed: Option<u64>,
    pub dirty_files: u64,
    pub uncommitted: UncommittedStat,
    /// `merged`, `pushed`, `discarded` — how the branch was finished.
    pub action: String,
    pub archived_at: Option<String>,
}

/// One project's archive, grouped by kind.
#[derive(Debug, Deserialize, Serialize)]
pub struct ArchiveListResult {
    pub plans: Vec<IssueRow>,
    pub worktrees: Vec<ArchivedWorktreeRow>,
}

/// One finished thing, in the one shape the archive lists every kind of
/// finished thing in: the keys another kind fills are `null` here.
#[derive(Debug, Deserialize, Serialize)]
pub struct ArchivedItem {
    /// `issue` or `branch`.
    pub kind: String,
    pub project_id: Option<String>,
    pub project: Option<String>,
    pub title: Option<String>,
    pub branch: Option<String>,
    pub state: Option<String>,
    pub action: Option<String>,
    pub finished_at: Option<String>,
    pub run_id: Option<String>,
    pub issue_id: Option<String>,
    pub stages: Option<u64>,
    pub worktree_id: Option<String>,
    pub worktree_path: Option<String>,
    pub head_sha: Option<String>,
    pub upstream: Option<String>,
    pub unpushed: Option<u64>,
    pub dirty_files: Option<u64>,
}

/// Everything finished, across every project, newest first.
#[derive(Debug, Deserialize, Serialize)]
pub struct ArchivedListResult {
    pub items: Vec<ArchivedItem>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CaptureListResult {
    pub captures: Vec<Capture>,
}

/// What a cancelled capture leaves behind: the id, and that it is gone.
#[derive(Debug, Deserialize, Serialize)]
pub struct CaptureCancelled {
    pub capture_id: String,
    pub cancelled: bool,
}

/// Which mode each agent family opens in.
#[derive(Debug, Deserialize, Serialize)]
pub struct AgentModesView {
    pub claude: String,
    pub codex: String,
}

/// Every account setting this bridge holds — what `settings.get` reads and
/// what `settings.set` answers with, so a client repaints from the set's own
/// reply.
#[derive(Debug, Deserialize, Serialize)]
pub struct SettingsResult {
    pub projects_dir: String,
    pub default_harness: String,
    pub agent_modes: AgentModesView,
    /// Deprecated alias, describing the concrete default harness.
    pub claude_mode: String,
    /// Deprecated alias, as `claude_mode` is.
    pub codex_mode: String,
    pub isolation: String,
    pub triage_enabled: bool,
    pub isolation_available: IsolationAvailabilityView,
}

/// One model a harness can be opened on.
#[derive(Debug, Deserialize, Serialize)]
pub struct ModelRow {
    pub id: String,
    pub label: String,
    pub supports_effort: bool,
    pub efforts: Vec<String>,
}

/// One harness and everything it can be asked for.
#[derive(Debug, Deserialize, Serialize)]
pub struct ProviderCatalog {
    pub id: String,
    pub label: String,
    pub efforts: Vec<String>,
    pub models: Vec<ModelRow>,
}

/// What a new agent can be started on. `models`/`efforts` are the default
/// harness's catalog, repeated at the top level for clients that predate
/// `providers`.
#[derive(Debug, Deserialize, Serialize)]
pub struct ModelsListResult {
    pub models: Vec<ModelRow>,
    pub efforts: Vec<String>,
    pub default_provider: String,
    pub agent_modes: AgentModesView,
    pub providers: Vec<ProviderCatalog>,
}

// ----------------------------------------------------------------- codes ---

/// The refusals this family can name, over the ones
/// [`ApiError::classify`](crate::api::ApiError::classify) already reads out of
/// the sentence. Board verbs refuse in prose written long before there were
/// codes, so the phrases are matched here rather than the sentences rewritten:
/// a message is the wire's, and changing one to win a code would be a change
/// to what every existing client shows its user.
const CONFLICTS: [&str; 5] = [
    "is already a git repository",
    "is not a git repository",
    "already became",
    "has already been answered",
    "the router has not asked anything",
];

/// A refusal the client can fix by asking differently: a value that is not
/// one of the ones on offer, or a request that names nothing to do. The three
/// harness names read alike because the two aliases and the harness itself
/// are one setting.
const INVALID: [&str; 11] = [
    "invalid project name",
    "nothing to set",
    "text is empty",
    "no option was offered at position",
    "was offered with that question",
    "is not a destination",
    "unknown isolation ",
    "unknown agent_modes",
    "unknown claude_mode",
    "unknown codex_mode",
    "unknown default_harness",
];

/// A refusal a client cannot act on and a retry cannot fix: the volume, not
/// the request.
const UNAVAILABLE: [&str; 1] = ["isolation is unavailable"];

fn says(message: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|phrase| message.contains(phrase))
}

/// Name the refusal this family raised, where the sentence says what
/// [`ApiError::classify`](crate::api::ApiError::classify) cannot read from
/// its general rules. Anything unmatched keeps the code classify gave it.
fn refine(error: ApiError) -> ApiError {
    let message = error.message();
    if says(message, &UNAVAILABLE) {
        return ApiError::unavailable(message);
    }
    if says(message, &CONFLICTS) {
        return ApiError::conflict(message, None);
    }
    if says(message, &INVALID) {
        return ApiError::invalid_params(message);
    }
    error
}

// -------------------------------------------------------------- handlers ---

fn board_list(app: &mut AppState, _params: NoParams) -> Result<Answer<BoardListResult>, ApiError> {
    answer(Ok(app.board_list()))
}

fn archive_list(
    app: &mut AppState,
    params: ProjectParams,
) -> Result<Answer<ArchiveListResult>, ApiError> {
    answer(app.archive_list(&params.wire())).map_err(refine)
}

fn archived_list(
    app: &mut AppState,
    _params: NoParams,
) -> Result<Answer<ArchivedListResult>, ApiError> {
    answer(Ok(app.archived_list()))
}

fn project_list(
    app: &mut AppState,
    _params: NoParams,
) -> Result<Answer<ProjectListResult>, ApiError> {
    answer(Ok(app.defer_project_list()))
}

fn project_add(
    app: &mut AppState,
    params: ProjectAddParams,
) -> Result<Answer<ProjectRow>, ApiError> {
    answer(app.project_add(&params.wire())).map_err(refine)
}

fn project_create(
    app: &mut AppState,
    params: ProjectCreateParams,
) -> Result<Answer<ProjectRow>, ApiError> {
    answer(app.project_create(&params.wire())).map_err(refine)
}

fn project_clone(
    app: &mut AppState,
    params: ProjectCloneParams,
) -> Result<Answer<ProjectRow>, ApiError> {
    answer(app.project_clone(&params.wire())).map_err(refine)
}

fn project_init_git(
    app: &mut AppState,
    params: ProjectParams,
) -> Result<Answer<ProjectRow>, ApiError> {
    answer(app.project_init_git(&params.wire())).map_err(refine)
}

fn project_set_remote(
    app: &mut AppState,
    params: ProjectSetRemoteParams,
) -> Result<Answer<ProjectRow>, ApiError> {
    answer(app.project_set_remote(&params.wire())).map_err(refine)
}

fn project_set_isolation(
    app: &mut AppState,
    params: ProjectSetIsolationParams,
) -> Result<Answer<ProjectRow>, ApiError> {
    answer(app.project_set_isolation(&params.wire())).map_err(refine)
}

fn capture_create(
    app: &mut AppState,
    params: CaptureCreateParams,
) -> Result<Answer<Capture>, ApiError> {
    answer(app.capture_create(&params.wire())).map_err(refine)
}

fn capture_list(
    app: &mut AppState,
    _params: NoParams,
) -> Result<Answer<CaptureListResult>, ApiError> {
    answer(Ok(app.capture_list()))
}

fn capture_get(app: &mut AppState, params: CaptureParams) -> Result<Answer<Capture>, ApiError> {
    answer(app.capture_get(&params.wire())).map_err(refine)
}

fn capture_answer(
    app: &mut AppState,
    params: CaptureAnswerParams,
) -> Result<Answer<Capture>, ApiError> {
    answer(app.capture_answer(&params.wire())).map_err(refine)
}

fn capture_reroute(
    app: &mut AppState,
    params: CaptureRerouteParams,
) -> Result<Answer<Capture>, ApiError> {
    answer(app.capture_reroute(&params.wire())).map_err(refine)
}

fn capture_cancel(
    app: &mut AppState,
    params: CaptureParams,
) -> Result<Answer<CaptureCancelled>, ApiError> {
    answer(app.capture_cancel(&params.wire())).map_err(refine)
}

fn settings_get(app: &mut AppState, _params: NoParams) -> Result<Answer<SettingsResult>, ApiError> {
    answer(Ok(app.settings_get()))
}

fn settings_set(
    app: &mut AppState,
    params: SettingsSetParams,
) -> Result<Answer<SettingsResult>, ApiError> {
    answer(app.settings_set(&params.wire())).map_err(refine)
}

fn models_list(
    app: &mut AppState,
    _params: NoParams,
) -> Result<Answer<ModelsListResult>, ApiError> {
    answer(Ok(app.models_list()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trips(method: &str) {
        crate::api::v1::testing::fixture_round_trips(methods(), method);
    }

    #[test]
    fn the_board_list_fixture_round_trips() {
        round_trips("board.list");
    }

    #[test]
    fn the_archive_list_fixture_round_trips() {
        round_trips("archive.list");
    }

    #[test]
    fn the_archived_list_fixture_round_trips() {
        round_trips("archived.list");
    }

    #[test]
    fn the_project_list_fixture_round_trips() {
        round_trips("project.list");
    }

    #[test]
    fn the_project_add_fixture_round_trips() {
        round_trips("project.add");
    }

    #[test]
    fn the_project_create_fixture_round_trips() {
        round_trips("project.create");
    }

    #[test]
    fn the_project_clone_fixture_round_trips() {
        round_trips("project.clone");
    }

    #[test]
    fn the_project_init_git_fixture_round_trips() {
        round_trips("project.init_git");
    }

    #[test]
    fn the_project_set_remote_fixture_round_trips() {
        round_trips("project.set_remote");
    }

    #[test]
    fn the_project_set_isolation_fixture_round_trips() {
        round_trips("project.set_isolation");
    }

    #[test]
    fn the_capture_create_fixture_round_trips() {
        round_trips("capture.create");
    }

    #[test]
    fn the_capture_list_fixture_round_trips() {
        round_trips("capture.list");
    }

    #[test]
    fn the_capture_get_fixture_round_trips() {
        round_trips("capture.get");
    }

    #[test]
    fn the_capture_answer_fixture_round_trips() {
        round_trips("capture.answer");
    }

    #[test]
    fn the_capture_reroute_fixture_round_trips() {
        round_trips("capture.reroute");
    }

    #[test]
    fn the_capture_cancel_fixture_round_trips() {
        round_trips("capture.cancel");
    }

    #[test]
    fn the_settings_get_fixture_round_trips() {
        round_trips("settings.get");
    }

    #[test]
    fn the_settings_set_fixture_round_trips() {
        round_trips("settings.set");
    }

    #[test]
    fn the_models_list_fixture_round_trips() {
        round_trips("models.list");
    }

    /// Clearing a project's isolation is `null`, and it has to reach the
    /// implementation as `null` rather than as an absent key — which is what
    /// leaving `skip_serializing_if` off this one field buys.
    #[test]
    fn a_cleared_isolation_reaches_the_implementation_as_null() {
        let params: ProjectSetIsolationParams = serde_json::from_value(
            serde_json::json!({ "project_id": "proj-1", "isolation": null }),
        )
        .expect("a clear parses");
        assert_eq!(
            params.wire(),
            serde_json::json!({ "project_id": "proj-1", "isolation": null })
        );
        let named: ProjectSetIsolationParams = serde_json::from_value(
            serde_json::json!({ "project_id": "proj-1", "isolation": "cow" }),
        )
        .expect("a choice parses");
        assert_eq!(
            named.wire(),
            serde_json::json!({ "project_id": "proj-1", "isolation": "cow" })
        );
    }

    /// Naming no isolation at all is a missing param, not a clear.
    #[test]
    fn an_unnamed_isolation_is_a_missing_param() {
        let refused = super::super::parse_params::<ProjectSetIsolationParams>(
            &serde_json::json!({ "project_id": "proj-1" }),
        )
        .expect_err("naming nothing is refused");
        assert_eq!(refused.message(), "missing required param: isolation");
        assert_eq!(refused.code(), "invalid_params");
    }

    /// A settings patch carries what the client named and nothing else, so a
    /// set of one field cannot quietly rewrite the rest of the account.
    #[test]
    fn a_settings_patch_carries_only_what_it_named() {
        let params: SettingsSetParams =
            serde_json::from_value(serde_json::json!({ "triage_enabled": false, "unknown": 1 }))
                .expect("an unknown field is ignored");
        assert_eq!(
            params.wire(),
            serde_json::json!({ "triage_enabled": false })
        );
    }

    /// A `null` a client named is not the same as a field it left out: the
    /// account refuses the first and ignores the second, and the params type
    /// has to carry the difference that far.
    #[test]
    fn a_settings_patch_keeps_a_null_the_client_named() {
        let params: SettingsSetParams = serde_json::from_value(
            serde_json::json!({ "agent_modes": null, "triage_enabled": true }),
        )
        .expect("a named null parses");
        assert_eq!(
            params.wire(),
            serde_json::json!({ "agent_modes": null, "triage_enabled": true })
        );
    }

    #[test]
    fn a_refusal_this_family_can_name_gets_its_code() {
        let cases = [
            ("project is already a git repository", "conflict"),
            (
                "project is not a git repository; initialize Git first",
                "conflict",
            ),
            (
                "capture.cancel: this capture already became issue; cancel that instead",
                "conflict",
            ),
            (
                "the router has not asked anything about this capture",
                "conflict",
            ),
            ("invalid project name: \"a/b\"", "invalid_params"),
            ("settings.set: nothing to set", "invalid_params"),
            (
                "capture.create: text is empty — there is nothing to keep",
                "invalid_params",
            ),
            (
                "unknown isolation \"btrfs\" (expected \"worktree\" or \"cow\")",
                "invalid_params",
            ),
            (
                "unknown default_harness \"gpt\" (expected \"claude_adk\", \"claude\")",
                "invalid_params",
            ),
            (
                "copy-on-write isolation is unavailable: no reflink here; locked to worktrees",
                "unavailable",
            ),
            ("unknown project: proj-9", "not_found"),
            ("unknown capture_id: capture-9", "not_found"),
            ("the store is gone", "internal"),
        ];
        for (message, code) in cases {
            let named = refine(ApiError::classify(message.to_string()));
            assert_eq!(named.code(), code, "{message}");
            assert_eq!(named.message(), message, "a code never rewrites the wire");
        }
    }
}
