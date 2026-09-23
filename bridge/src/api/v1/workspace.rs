//! The workspace family: the durable multi-source checkout the SPA opens
//! work in (`workspace.list` / `workspace.get` / `workspace.create` /
//! `workspace.retry` / `workspace.finish`), the two verbs its settings sheet
//! calls (`workspace.rename` / `workspace.delete`), the conversation owner a
//! workspace mints on demand (`workspace.ensure_conversation`), and the Git
//! initialization surface for a source that is not a repository yet
//! (`workspace.git_init_options` / `workspace.init_git`).
//!
//! `rename` moves the record's pretty name and nothing else — not the folder,
//! not the branches — and answers the detail `get` answers, so one call
//! repaints. `delete` is the opposite end of `create`: it answers
//! `{workspace_id, deleted}` under the lock and publishes the same shape from
//! the drain, so it needs no placeholder.
//!
//! Same shape as `board.rs`: the implementations under `app/workspaces/` are
//! untouched — each handler resolves its typed params, hands them to the
//! implementation that already exists, and names the shape that
//! implementation answers in.
//!
//! Four of the eight defer. `workspace.init_git` answers the usual
//! `Value::Null` placeholder; `create`, `retry` and `finish` acknowledge under
//! the lock with `{"workspace_id", "pending": true}`, which
//! [`deferral_placeholder`] reads as the same "nothing to check yet" — the
//! real value is what the drain publishes, and [`Handler::check_result`] holds
//! it to the type named here. `create` and `retry` publish a [`WorkspaceRow`],
//! `finish` publishes the [`WorkspaceFinishResult`] the run/branch/worktree
//! spellings of Done already answer, and `init_git` publishes
//! [`WorkspaceInitGitResult`].
//!
//! On the nullable fields: `workspace_json` and the `workspace.get` detail
//! write their absences as explicit `null`s rather than leaving the key out,
//! so the optionals here deliberately carry no `skip_serializing_if` — an
//! absent key and a `null` one are NOT the same to the SPA's workspace
//! surfaces, which read `entity_id in payload` to tell a bridge that answers
//! ownership from one that does not. The optionals that really are absent
//! (the Git init option's `path`, `reason` and `needs_reconciliation`, and a
//! finished repository's `reason`) do skip.

use super::lifecycle::{RunAgentChoiceParams, RunView, ThreadPayload, ThreadWindowParams};
use super::thread::AgentDigest;
use super::{answer, deferral_placeholder, Answer, Handler, WireParams};
use crate::api::v1::lifecycle::WorkspaceFinishResult;
use crate::api::ApiError;
use crate::app::AppState;
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};

/// The verbs this family serves.
pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!(
            "workspace.list",
            workspace_list,
            WorkspaceListParams,
            WorkspaceListResult
        ),
        v1_method!(
            "workspace.get",
            workspace_get,
            WorkspaceGetParams,
            WorkspaceDetail
        ),
        v1_method!(
            "workspace.create",
            workspace_create,
            WorkspaceCreateParams,
            WorkspaceRow
        ),
        v1_method!(
            "workspace.retry",
            workspace_retry,
            WorkspaceIdParams,
            WorkspaceRow
        ),
        v1_method!(
            "workspace.ensure_conversation",
            workspace_ensure_conversation,
            WorkspaceEnsureConversationParams,
            WorkspaceConversation
        ),
        v1_method!(
            "workspace.git_init_options",
            workspace_git_init_options,
            WorkspaceSourceParams,
            GitInitOptions
        ),
        v1_method!(
            "workspace.init_git",
            workspace_init_git,
            WorkspaceInitGitParams,
            WorkspaceInitGitResult
        ),
        v1_method!(
            "workspace.finish",
            workspace_finish,
            WorkspaceIdParams,
            WorkspaceFinishResult
        ),
        v1_method!(
            "workspace.add_directory",
            workspace_add_directory,
            WorkspaceAddDirectoryParams,
            WorkspaceDetail
        ),
        v1_method!(
            "workspace.remove_directory",
            workspace_remove_directory,
            WorkspaceRemoveDirectoryParams,
            WorkspaceDetail
        ),
        v1_method!(
            "workspace.rename",
            workspace_rename,
            WorkspaceRenameParams,
            WorkspaceDetail
        ),
        v1_method!(
            "workspace.delete",
            workspace_delete,
            WorkspaceIdParams,
            WorkspaceDeleteResult
        ),
    ]
}

// ---------------------------------------------------------------- params ---

/// The workspaces of one project, or of every project when no project is
/// named — the feed reads it unfiltered.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct WorkspaceListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// A workspace named and nothing else asked of it: `workspace.retry` and
/// `workspace.delete`.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceIdParams {
    pub workspace_id: String,
}

/// One more directory in a workspace that is already standing: a project
/// source it was not cut with, a path on this device, or a remote to clone —
/// exactly one of the three.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceAddDirectoryParams {
    pub workspace_id: String,
    /// A source of the workspace's own project. Mutually exclusive with the
    /// other two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Absolute or `~`-relative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A clone url. The clone lands in the workspace and is its own
    /// repository; nothing about the project changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// The branch a Git directory is cut from; the source's own when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
}

/// One directory leaves a workspace. `directory_id` is the directory's own id
/// or the project source's, the way every directory-scoped verb reads it.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceRemoveDirectoryParams {
    pub workspace_id: String,
    pub directory_id: String,
}

/// The workspace's new human-facing name. Trimmed and refused empty by the
/// implementation; the folder on disk and the branches in it are NOT renamed,
/// so nothing here names a path.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceRenameParams {
    pub workspace_id: String,
    pub name: String,
}

/// The workspace, and the conversation slice its answer carries.
///
/// The detail hands its whole params on to `run.get` once it has resolved the
/// workspace's conversation owner, so the thread window a client sends for a
/// run is the one it sends here — including the `agent_id` a workspace with
/// no owner refuses by name.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceGetParams {
    pub workspace_id: String,
    #[serde(flatten)]
    pub view: ThreadWindowParams,
}

/// Cut a workspace over every source the project has.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceCreateParams {
    pub project_id: String,
    /// Trimmed; `workspace` when absent or blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `worktree` or `rift` (`cow` is read as `rift`). The project's own
    /// setting, then the account's, when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
}

/// Give this workspace a conversation owner, and name what the agents on it
/// run.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceEnsureConversationParams {
    pub workspace_id: String,
    /// The model the conversation is minted with. Only read the first time —
    /// a workspace that already owns a conversation answers with it.
    #[serde(flatten)]
    pub choice: RunAgentChoiceParams,
}

/// One directory of one workspace: `workspace.git_init_options`.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceSourceParams {
    pub workspace_id: String,
    /// The project source's id, or the workspace directory's own.
    pub source_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceInitGitParams {
    pub workspace_id: String,
    pub source_id: String,
    /// `workspace` (the copy), `source` (what it was copied from), or `both`.
    pub target: String,
}

// --------------------------------------------------------------- results ---

/// One directory inside a workspace: which project source it stands for,
/// where it landed, and what became of it.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceDirectoryRow {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub path: String,
    pub is_git: bool,
    /// The branch the checkout is on; `null` for a directory that is not a
    /// repository.
    pub branch: Option<String>,
    /// `worktree` or `rift` — what the copy actually got, which is not always
    /// what was asked for. `null` before provisioning resolved it.
    pub isolation: Option<String>,
    /// `pending`, `ready` or `failed`.
    pub status: String,
    /// Why provisioning this directory failed. `null` when it did not.
    pub error: Option<String>,
}

/// A workspace as every workspace verb answers it.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceRow {
    pub id: String,
    /// The same id, under the name every workspace param spells it.
    pub workspace_id: String,
    pub project_id: String,
    pub name: String,
    pub root: String,
    /// `provisioning`, `ready`, `finished` or `failed`.
    pub status: String,
    /// Whether an agent created this workspace. Older peers omit the field.
    #[serde(default)]
    pub created_by_agent: bool,
    /// When a clean-only Done archived the workspace; otherwise `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    pub directories: Vec<WorkspaceDirectoryRow>,
}

/// A workspace as the list answers it: the row, and the conversation entity
/// that owns it — `null` for a workspace nobody has spoken in — so a client
/// standing on the list files the workspace's git under that entity without
/// asking for the detail.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceListRow {
    #[serde(flatten)]
    pub workspace: WorkspaceRow,
    pub entity_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceListResult {
    pub workspaces: Vec<WorkspaceListRow>,
}

/// A workspace with its conversation, when it has one.
///
/// All five conversation keys are always present: a workspace with no owner
/// answers them `null` (and `agents` empty), which is how the rail tells "this
/// bridge answers ownership and there is none" from "this bridge is too old to
/// answer ownership at all".
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceDetail {
    #[serde(flatten)]
    pub workspace: WorkspaceRow,
    /// The run that owns this workspace's root, under the name the rail reads
    /// entities by.
    pub entity_id: Option<String>,
    /// The same id, under the name the run verbs take.
    pub run_id: Option<String>,
    pub agents: Vec<AgentDigest>,
    /// The owner's conversation, exactly as `run.get` cut it.
    pub thread: ThreadPayload,
    /// The owner's whole run view.
    pub run: Option<Box<RunView>>,
}

/// What `workspace.delete` answers, the acknowledgement and the drain's own
/// value alike — the whole removal is one fact, and the client that asked for
/// it only needs to know it happened.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceDeleteResult {
    pub workspace_id: String,
    pub deleted: bool,
}

/// The conversation owner a workspace has, or the one it just minted.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceConversation {
    pub workspace_id: String,
    pub entity_id: String,
    /// The same id, under the name the run verbs take.
    pub run_id: String,
}

/// Whether one of the two directories `workspace.init_git` can target could
/// take a `git init`, and what is already there.
#[derive(Debug, Deserialize, Serialize)]
pub struct GitInitOption {
    /// Absent when the directory could not be resolved at all — there is no
    /// path to name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub available: bool,
    /// A repository with a commit is already here.
    pub is_git: bool,
    /// Git is on disk but the record disagrees, or the repository has no
    /// commit yet: initializing would reconcile rather than create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_reconciliation: Option<bool>,
    /// Why it is not available. Absent when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitInitOptions {
    pub workspace_id: String,
    pub source_id: String,
    /// The workspace's own copy of the directory.
    pub workspace: GitInitOption,
    /// The project source the copy was made from.
    pub source: GitInitOption,
}

/// What one target of one `workspace.init_git` came to.
#[derive(Debug, Deserialize, Serialize)]
pub struct GitInitOutcome {
    /// `workspace` or `source`.
    pub target: String,
    /// `initialized`, `already_initialized` or `failed`.
    pub status: String,
    pub is_git: bool,
    /// Why this target failed. Absent when it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The project source a `workspace.init_git` recorded Git on, as much of it
/// as the Git surface reads.
#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectSourceGit {
    pub id: String,
    pub is_git: bool,
    pub base_branch: String,
}

/// One outcome per target, and the two records the initialization rewrote.
///
/// `workspace` and `source` are always present and `null` when the record
/// could not be re-read, which is how a client tells "nothing to refresh"
/// from "refresh this".
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkspaceInitGitResult {
    pub results: Vec<GitInitOutcome>,
    pub workspace: Option<WorkspaceRow>,
    pub source: Option<ProjectSourceGit>,
}

// ----------------------------------------------------------------- codes ---

/// Something else holds the filesystem, and the same request sent again once
/// it lets go may well succeed. The only retryable class there is.
const BUSY: [&str; 1] = ["another filesystem operation is still running"];

/// The request was legible and the workspace's own state said no.
const CONFLICT: [&str; 15] = [
    // Adding and removing a directory: what the workspace is, and what is
    // standing in the way of the folder going.
    "adopted checkouts are not Build's to change",
    "is already a directory in workspace",
    "Cannot remove a workspace directory",
    // `workspace.ensure_conversation: workspace is finished`, and its
    // provisioning and failed spellings.
    "workspace is ",
    "workspace provisioning is already running",
    "workspace has no failed provisioning to retry",
    "adopted workspaces require no provisioning",
    "workspace must finish provisioning successfully",
    // Done, refused because the work is still only here.
    "workspace.finish is not available yet",
    "project has no sources",
    // Rename and delete: the workspace is not Build's to rewrite or remove,
    // or something standing in it has to stop first.
    "adopted workspaces are named by their own checkout",
    "Build cannot remove an adopted checkout",
    "Wait for workspace provisioning to finish",
    "Stop running agents before deleting the workspace",
    "Cannot delete a workspace",
];

/// A word in the request is not one this bridge knows. `unknown <thing>`
/// otherwise reads as a missing entity, which these are not.
const INVALID: [&str; 6] = [
    "name a source_id, a path or a remote",
    "duplicate source mount:",
    "unknown isolation:",
    "unknown git init target:",
    "unknown agent provider:",
    "workspace.rename: name cannot be empty",
];

/// Name the refusal this family raised, where the sentence says what
/// [`ApiError::classify`](crate::api::ApiError::classify) cannot read from its
/// general rules. Anything unmatched keeps the code classify gave it. The
/// message is never rewritten — only the code beside it is decided.
fn refine(error: ApiError) -> ApiError {
    let message = error.message().to_string();
    if BUSY.iter().any(|marker| message.contains(marker)) {
        return ApiError::busy(message);
    }
    if CONFLICT.iter().any(|marker| message.contains(marker)) {
        return ApiError::conflict(message, None);
    }
    if INVALID.iter().any(|marker| message.contains(marker)) {
        return ApiError::invalid_params(message);
    }
    error
}

// -------------------------------------------------------------- handlers ---

fn workspace_list(
    app: &mut AppState,
    params: WorkspaceListParams,
) -> Result<Answer<WorkspaceListResult>, ApiError> {
    answer(app.workspace_list(&params.wire())).map_err(refine)
}

fn workspace_get(
    app: &mut AppState,
    params: WorkspaceGetParams,
) -> Result<Answer<WorkspaceDetail>, ApiError> {
    answer(app.workspace_get(&params.wire())).map_err(refine)
}

fn workspace_create(
    app: &mut AppState,
    params: WorkspaceCreateParams,
) -> Result<Answer<WorkspaceRow>, ApiError> {
    answer(
        app.workspace_create(&params.wire())
            .map(deferral_placeholder),
    )
    .map_err(refine)
}

fn workspace_retry(
    app: &mut AppState,
    params: WorkspaceIdParams,
) -> Result<Answer<WorkspaceRow>, ApiError> {
    answer(
        app.workspace_retry(&params.wire())
            .map(deferral_placeholder),
    )
    .map_err(refine)
}

fn workspace_ensure_conversation(
    app: &mut AppState,
    params: WorkspaceEnsureConversationParams,
) -> Result<Answer<WorkspaceConversation>, ApiError> {
    answer(app.workspace_ensure_conversation(&params.wire())).map_err(refine)
}

fn workspace_git_init_options(
    app: &mut AppState,
    params: WorkspaceSourceParams,
) -> Result<Answer<GitInitOptions>, ApiError> {
    answer(app.workspace_git_init_options(&params.wire())).map_err(refine)
}

fn workspace_init_git(
    app: &mut AppState,
    params: WorkspaceInitGitParams,
) -> Result<Answer<WorkspaceInitGitResult>, ApiError> {
    answer(app.workspace_init_git(&params.wire())).map_err(refine)
}

fn workspace_finish(
    app: &mut AppState,
    params: WorkspaceIdParams,
) -> Result<Answer<WorkspaceFinishResult>, ApiError> {
    answer(
        app.workspace_finish(&params.wire())
            .map(deferral_placeholder),
    )
    .map_err(refine)
}

/// The checkout, the copy or the clone runs off the app mutex, so both of
/// these answer the placeholder and the drain publishes the workspace detail
/// `workspace.get` answers — one read repaints everything naming a directory.
fn workspace_add_directory(
    app: &mut AppState,
    params: WorkspaceAddDirectoryParams,
) -> Result<Answer<WorkspaceDetail>, ApiError> {
    answer(
        app.workspace_add_directory(&params.wire())
            .map(deferral_placeholder),
    )
    .map_err(refine)
}

fn workspace_remove_directory(
    app: &mut AppState,
    params: WorkspaceRemoveDirectoryParams,
) -> Result<Answer<WorkspaceDetail>, ApiError> {
    answer(
        app.workspace_remove_directory(&params.wire())
            .map(deferral_placeholder),
    )
    .map_err(refine)
}

fn workspace_rename(
    app: &mut AppState,
    params: WorkspaceRenameParams,
) -> Result<Answer<WorkspaceDetail>, ApiError> {
    answer(app.workspace_rename(&params.wire())).map_err(refine)
}

/// The removal itself runs off the app mutex, but the shape never changes:
/// the acknowledgement here and the value the drain publishes are the same
/// `{workspace_id, deleted}`, so no placeholder is needed.
fn workspace_delete(
    app: &mut AppState,
    params: WorkspaceIdParams,
) -> Result<Answer<WorkspaceDeleteResult>, ApiError> {
    answer(app.workspace_delete(&params.wire())).map_err(refine)
}

// ----------------------------------------------------------------- tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::v1::parse_params;

    fn round_trips(method: &str) {
        crate::api::v1::testing::fixture_round_trips(methods(), method);
    }

    #[test]
    fn the_workspace_list_fixture_round_trips() {
        round_trips("workspace.list");
    }

    #[test]
    fn the_workspace_get_fixture_round_trips() {
        round_trips("workspace.get");
    }

    #[test]
    fn the_workspace_create_fixture_round_trips() {
        round_trips("workspace.create");
    }

    #[test]
    fn the_workspace_retry_fixture_round_trips() {
        round_trips("workspace.retry");
    }

    #[test]
    fn the_workspace_ensure_conversation_fixture_round_trips() {
        round_trips("workspace.ensure_conversation");
    }

    #[test]
    fn the_workspace_git_init_options_fixture_round_trips() {
        round_trips("workspace.git_init_options");
    }

    #[test]
    fn the_workspace_init_git_fixture_round_trips() {
        round_trips("workspace.init_git");
    }

    #[test]
    fn the_workspace_finish_fixture_round_trips() {
        round_trips("workspace.finish");
    }

    #[test]
    fn the_workspace_add_directory_fixture_round_trips() {
        round_trips("workspace.add_directory");
    }

    #[test]
    fn the_workspace_remove_directory_fixture_round_trips() {
        round_trips("workspace.remove_directory");
    }

    #[test]
    fn the_workspace_rename_fixture_round_trips() {
        round_trips("workspace.rename");
    }

    #[test]
    fn the_workspace_delete_fixture_round_trips() {
        round_trips("workspace.delete");
    }

    /// Renaming needs both words. A name-less rename would otherwise reach the
    /// implementation and be refused there in different words.
    #[test]
    fn a_rename_missing_its_name_is_a_missing_param() {
        let refused = parse_params::<WorkspaceRenameParams>(&serde_json::json!({
            "workspace_id": "ws-1",
        }))
        .expect_err("naming no name is refused");
        assert_eq!(refused.message(), "missing required param: name");
        assert_eq!(refused.code(), "invalid_params");
    }

    /// The detail hands its params on to `run.get`, so the thread window has
    /// to survive the typed params — a naive `{workspace_id}` would drop the
    /// cursor the rail sends and answer the whole conversation every poll.
    #[test]
    fn the_detail_carries_the_thread_window_on_to_the_run_read() {
        let params: WorkspaceGetParams = serde_json::from_value(serde_json::json!({
            "workspace_id": "ws-1",
            "agent_id": "agent-2",
            "conversation_id": "conv-3",
            "thread_after_sequence": 41,
            "thread_limit": 25,
            "unknown_to_this_bridge": true,
        }))
        .expect("a windowed read parses");
        assert_eq!(
            params.wire(),
            serde_json::json!({
                "workspace_id": "ws-1",
                "agent_id": "agent-2",
                "conversation_id": "conv-3",
                "thread_after_sequence": 41,
                "thread_limit": 25,
            })
        );
    }

    /// Minting a conversation names the model it runs on, exactly as the run
    /// verbs do; dropping these would silently swap the account default in.
    #[test]
    fn ensuring_a_conversation_carries_the_model_choice() {
        let params: WorkspaceEnsureConversationParams = serde_json::from_value(serde_json::json!({
            "workspace_id": "ws-1",
            "provider": "codex",
            "model": "gpt-5",
            "effort": "medium",
        }))
        .expect("a choice parses");
        assert_eq!(
            params.wire(),
            serde_json::json!({
                "workspace_id": "ws-1",
                "provider": "codex",
                "model": "gpt-5",
                "effort": "medium",
            })
        );
    }

    /// Listing every project's workspaces is naming no project, and it has to
    /// reach the implementation as an absent key rather than a `null` — the
    /// filter reads `as_str`, so either works, but the wire says what it means.
    #[test]
    fn listing_without_a_project_names_none() {
        let params: WorkspaceListParams =
            serde_json::from_value(serde_json::json!({})).expect("naming nothing parses");
        assert_eq!(params.wire(), serde_json::json!({}));
    }

    #[test]
    fn a_workspace_verb_missing_its_id_is_a_missing_param() {
        let refused = parse_params::<WorkspaceIdParams>(&serde_json::json!({}))
            .expect_err("naming nothing is refused");
        assert_eq!(refused.message(), "missing required param: workspace_id");
        assert_eq!(refused.code(), "invalid_params");
    }

    #[test]
    fn a_refusal_this_family_can_name_gets_its_code() {
        let cases = [
            ("another filesystem operation is still running", "busy"),
            (
                "workspace.ensure_conversation: workspace is finished",
                "conflict",
            ),
            ("workspace provisioning is already running", "conflict"),
            ("workspace has no failed provisioning to retry", "conflict"),
            (
                "workspace.retry: adopted workspaces require no provisioning",
                "conflict",
            ),
            (
                "workspace must finish provisioning successfully before it can be finished",
                "conflict",
            ),
            ("workspace.create: project has no sources", "conflict"),
            ("unknown isolation: btrfs", "invalid_params"),
            ("unknown git init target: everything", "invalid_params"),
            ("unknown agent provider: gpt", "invalid_params"),
            ("workspace.rename: name cannot be empty", "invalid_params"),
            (
                "workspace.rename: adopted workspaces are named by their own checkout",
                "conflict",
            ),
            (
                "Build cannot remove an adopted checkout. Only workspaces Build created can be deleted.",
                "conflict",
            ),
            (
                "Wait for workspace provisioning to finish before deleting the workspace",
                "conflict",
            ),
            (
                "Stop running agents before deleting the workspace",
                "conflict",
            ),
            (
                "Cannot delete a workspace containing a source repository",
                "conflict",
            ),
            ("unknown workspace_id: ws-9", "not_found"),
            ("unknown source_id source-9 in workspace ws-1", "not_found"),
            ("unknown project_id: proj-9", "not_found"),
            ("resolve workspace root /gone: No such file", "internal"),
        ];
        for (message, code) in cases {
            let named = refine(ApiError::classify(message.to_string()));
            assert_eq!(named.code(), code, "{message}");
            assert_eq!(named.message(), message, "a code never rewrites the wire");
        }
    }
}
