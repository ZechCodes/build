//! The git family: every `git.*` verb, the `fs.*` reads and the one write,
//! and the diff reads that render a checkout's uncommitted work
//! (`worktree.diff`, `project.diff`, `run.diff`, `run.stage_diff`,
//! `issue.diff`, `issue.stage_diff`).
//!
//! This is the converted family — the pattern the other four follow. The git
//! itself is untouched: each handler resolves its typed params, hands them to
//! the implementation that already exists under `app/`, and names the shape
//! that implementation answers in. Every `git.*` verb and every diff read
//! defers its work to the off-lock drain, so what the handler returns is the
//! placeholder [`Answer`] documents; the `fs.*` verbs answer inline.
//!
//! Optionals: `skip_serializing_if` throughout, so a field's absence and its
//! `null` mean the same to every client, exactly as step 2.2 requires.

use super::{answer, Answer, Handler, WireParams};
use crate::api::ApiError;
use crate::app::AppState;
use crate::body_page::{BodyRange, BodySpan};
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The verbs this family serves.
pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!("git.log", git_log, GitLogParams, GitLogResult),
        v1_method!("git.show", git_show, GitShowParams, CommitDetail),
        v1_method!("git.status", git_status, GitStatusParams, GitStatusResult),
        v1_method!("git.diff", git_diff, GitDiffParams, GitDiffResult),
        v1_method!(
            "git.changeset_diff",
            git_changeset_diff,
            ChangesetDiffParams,
            ChangesetDiffResult
        ),
        v1_method!("git.stage", git_stage, GitPathsParams, StatusPayload),
        v1_method!("git.unstage", git_unstage, GitPathsParams, StatusPayload),
        v1_method!("git.discard", git_discard, GitPathsParams, StatusPayload),
        v1_method!("git.commit", git_commit, GitCommitParams, GitCommitResult),
        v1_method!("git.fetch", git_fetch, ScopeParams, StatusPayload),
        v1_method!("git.pull", git_pull, GitPullParams, StatusPayload),
        v1_method!("git.push", git_push, GitPushParams, StatusPayload),
        v1_method!("git.stash", git_stash, ScopeParams, StatusPayload),
        v1_method!("git.stash_pop", git_stash_pop, ScopeParams, StatusPayload),
        v1_method!(
            "git.merge_abort",
            git_merge_abort,
            ScopeParams,
            StatusPayload
        ),
        v1_method!(
            "git.branches",
            git_branches,
            BranchScopeParams,
            BranchListResult
        ),
        v1_method!(
            "git.checkout",
            git_checkout,
            GitCheckoutParams,
            StatusPayload
        ),
        v1_method!(
            "git.branch_delete",
            git_branch_delete,
            GitBranchDeleteParams,
            BranchListResult
        ),
        v1_method!("git.refs", git_refs, ScopeParams, RefListResult),
        v1_method!(
            "git.checkout_ref",
            git_checkout_ref,
            GitCheckoutRefParams,
            StatusPayload
        ),
        v1_method!(
            "git.unpushed",
            git_unpushed,
            GitUnpushedParams,
            GitUnpushedResult
        ),
        v1_method!("fs.list", fs_list, FsListParams, FsListResult),
        v1_method!("fs.mkdir", fs_mkdir, FsMkdirParams, FsMkdirResult),
        v1_method!("fs.tree", fs_tree, FsTreeParams, FsTreeResult),
        v1_method!("fs.read", fs_read, FsReadParams, FsFileResult),
        v1_method!("fs.write", fs_write, FsWriteParams, FsFileResult),
        v1_method!(
            "project.diff",
            project_diff,
            ProjectDiffParams,
            ProjectDiffResult
        ),
        v1_method!(
            "worktree.diff",
            worktree_diff,
            WorktreeDiffParams,
            WorktreeDiffResult
        ),
        v1_method!("run.diff", run_diff, RunDiffParams, RunDiffResult),
        v1_method!(
            "run.stage_diff",
            run_stage_diff,
            RunStageDiffParams,
            StageDiffResult
        ),
        v1_method!("issue.diff", issue_diff, IssueDiffParams, RunDiffResult),
        v1_method!(
            "issue.stage_diff",
            issue_stage_diff,
            IssueStageDiffParams,
            StageDiffResult
        ),
    ]
}

// ---------------------------------------------------------------- params ---

/// The checkout a `git.*` or `fs.*` verb acts on, named the only way a client
/// may name one: by id. The repo path always comes from server state.
/// `project_id` alone is the project's repository, `run_id` is a run's
/// worktree, `project_id` + `worktree_id` is one of the project's external
/// worktrees, and `workspace_id` + `source_id` is one directory of a
/// multi-source workspace.
///
/// The workspace pair is exclusive with the three legacy ids — the
/// implementation refuses a request naming both — and, for a `git.*` verb,
/// the source it names has to be a git one.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ScopeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    /// A multi-source workspace. Requires `source_id` beside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// One directory of that workspace. Requires `workspace_id` beside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// The conversation the client files this directory under: the SPA sends
    /// a workspace's `entity_id` with its first git directory. It does not
    /// pick the checkout; after a mutating verb the drain notes it changed,
    /// beside the ids above (`entity_ids_of` in `app/rpc.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
}

/// The scope of a verb that addresses the repository's branches rather than
/// one checkout's working tree: project, optionally narrowed to an external
/// worktree. A run's branch belongs to the run lifecycle, so no `run_id`.
#[derive(Debug, Deserialize, Serialize)]
pub struct BranchScopeParams {
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
}

/// How many commits a CURSORED `git.log` answers with when the caller names
/// no limit.
///
/// Small on purpose: a client that already holds a log asks what has landed
/// since, and the answer to that is almost always nothing. The browsing
/// default (30, unchanged) is for a reader who is about to scroll.
pub const LATEST_COMMITS: u64 = 20;

#[derive(Debug, Deserialize, Serialize)]
pub struct GitLogParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    /// Clamped to 1..=200 server-side; 30 when absent, or
    /// [`LATEST_COMMITS`] when `since` names a cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<u64>,
    /// The newest commit the client already holds: a 4-40 character
    /// lowercase hex object-id prefix, validated exactly as
    /// [`GitShowParams::hash`] is and never a revspec.
    ///
    /// The answer is then `since..HEAD`, newest first. A cursor HEAD cannot
    /// reach — a rebase, a reset, a hash from another checkout — is not a
    /// refusal: the answer is the latest page with `reset` set. So is a gap
    /// wider than the page, which comes back as the latest commits rather
    /// than as a fragment with nothing holding it to the cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

/// The most patch one commit's answer carries when the caller caps it.
///
/// A quarter of a megabyte is a large review diff and a small cache entry.
/// Past it a client is better served by the file list and a second call for
/// the patch it actually opens.
pub const COMMIT_PATCH_MAX_BYTES: u64 = 262_144;

#[derive(Debug, Deserialize, Serialize)]
pub struct GitShowParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    /// A 4–40 character lowercase hex object-id prefix, never a revspec.
    pub hash: String,
    /// The most patch this answer may carry, clamped server-side to
    /// `1024..=`[`COMMIT_PATCH_MAX_BYTES`]. A larger patch comes back as the
    /// commit's file headers with `truncated` set, and the client fetches the
    /// whole of it when a reviewer opens the commit. Absent leaves the
    /// 1 MiB wire cap alone. Refused beside `range`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// One page of the body (#95): the bytes from `offset`, whole lines of at
    /// most `bytes`, and a `range` in the answer saying where they sit.
    /// Absent reads the body whole, capped as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<BodyRange>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitStatusParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    /// The `status_key` the client is already painting; when it still names
    /// the working tree the answer is [`UnchangedStatus`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_status_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitDiffParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    /// 1 to 50 repo-relative paths, answered in request order — exactly one
    /// beside `range`.
    pub paths: Vec<String>,
    /// One page of the body (#95): the bytes from `offset`, whole lines of at
    /// most `bytes`, and a `range` in the answer saying where they sit.
    /// Absent reads the body whole, capped as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<BodyRange>,
}

/// `git.stage`, `git.unstage`, `git.discard` — the same required path list.
#[derive(Debug, Deserialize, Serialize)]
pub struct GitPathsParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitCommitParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitPullParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    /// `ff` (the default), `merge`, or `rebase`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitPushParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    /// `--force-with-lease`, never a bare force.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitCheckoutParams {
    #[serde(flatten)]
    pub scope: BranchScopeParams,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitBranchDeleteParams {
    #[serde(flatten)]
    pub scope: BranchScopeParams,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
}

/// `fs.list` browses the host's directories, before any project exists — the
/// one verb here with no scope at all.
#[derive(Debug, Deserialize, Serialize)]
pub struct FsListParams {
    /// Absolute or `~`-relative; the user's home when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `fs.mkdir` makes one folder by name inside a parent the user picked in the
/// directory browser, so it is scoped by host path like `fs.list`, not by id.
#[derive(Debug, Deserialize, Serialize)]
pub struct FsMkdirParams {
    /// Absolute or `~`-relative, and must already exist.
    pub parent: String,
    /// A single folder name: no separator, no `.` or `..`.
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FsTreeParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    /// Scope-relative; the scope root when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FsReadParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    pub path: String,
    /// One page of the file (#95): the answer carries the bytes from
    /// `offset`, whole lines of at most `bytes`, and `range` says where they
    /// sit. Absent reads the file whole, capped as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<BodyRange>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FsWriteParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    pub path: String,
    /// The revision `fs.read` answered with; a stale one is refused.
    pub expected_revision: String,
    pub content_b64: String,
}

/// `git.checkout_ref` names one exact ref out of what `git.refs` listed —
/// never a revspec the server would have to interpret.
#[derive(Debug, Deserialize, Serialize)]
pub struct GitCheckoutRefParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    /// The `full_ref` of a row `git.refs` answered.
    pub full_ref: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitUnpushedParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    /// The `diff_key` the caller already holds; the read answers
    /// [`UnchangedDiff`] rather than the patch when it still matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_diff_key: Option<String>,
    /// Whether the aggregate patch rides the answer. `false` asks for the
    /// shape without it — the commit list, the per-file rows and the stat —
    /// which is everything a list paints and a fraction of the bytes: a
    /// client filling a cache threw the patch away on arrival, and on a
    /// phone's relayed path that was most of a megabyte per connect.
    /// Absent means yes, so a client that has not heard of this is answered
    /// exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<bool>,
}

/// `git.changeset_diff` — the hunks of named paths out of the changeset the
/// scope's own whole-patch verb answers.
///
/// The scope is the same one every `git.*` verb takes, and which changeset it
/// names follows the whole-patch verb for that scope: `run_id` is the run
/// against its baseline, `project_id` + `worktree_id` an external checkout
/// against its merge base, `project_id` alone the project's uncommitted work.
#[derive(Debug, Deserialize, Serialize)]
pub struct ChangesetDiffParams {
    #[serde(flatten)]
    pub scope: ScopeParams,
    /// 1 to 50 repo-relative paths — the files the reader has open.
    pub paths: Vec<String>,
    /// The `diff_key` the caller already holds. The key names the WHOLE
    /// changeset, so a body fetched under it is still that file's body for
    /// as long as the key stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_diff_key: Option<String>,
    /// One page of ONE path's hunks (#95): `paths` names exactly one, and the
    /// answer's `patch` is the bytes from `offset`, whole lines of at most
    /// `bytes`, with a `range` saying where they sit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<BodyRange>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectDiffParams {
    pub project_id: String,
    /// Whether the patch text rides the answer. `false` asks for the shape a
    /// list paints and leaves the hunks to the surface that opens them. Absent
    /// means yes, so an older client is answered as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WorktreeDiffParams {
    pub project_id: String,
    pub worktree_id: String,
    /// Whether the patch text rides the answer. `false` asks for the shape a
    /// list paints — the stat and the per-file rows, with the `diff_key` that
    /// names the body — and leaves the hunks to the surface that opens them.
    /// A client filling a cache has no use for a body nobody is looking at,
    /// and on a phone's relayed path that body was most of a megabyte per
    /// connect. Absent means yes, so an older client is answered as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_diff_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunDiffParams {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_diff_key: Option<String>,
    /// Whether the patch text rides the answer. `false` asks for the shape a
    /// list paints — the stat and the per-file rows, with the `diff_key` that
    /// names the body — and leaves the hunks to the surface that opens them.
    /// A client filling a cache has no use for a body nobody is looking at,
    /// and on a phone's relayed path that body was most of a megabyte per
    /// connect. Absent means yes, so an older client is answered as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunStageDiffParams {
    pub run_id: String,
    pub stage_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IssueDiffParams {
    pub issue_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_diff_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IssueStageDiffParams {
    pub issue_id: String,
    pub stage_id: String,
}

// --------------------------------------------------------------- results ---

/// The line census a diff or a status sums to.
#[derive(Debug, Deserialize, Serialize)]
pub struct DiffStat {
    pub files_changed: u64,
    pub insertions: u64,
    pub deletions: u64,
}

/// One changed path in a diff listing: what it is and what it weighs, not
/// what it says.
///
/// The counts are here because a reader holds the row before it holds the
/// hunks — a `git` push and a cold pass carry the list with no patch — and
/// the `+`/`−` beside the path is drawn from them.
#[derive(Debug, Deserialize, Serialize)]
pub struct DiffFileRow {
    pub path: String,
    /// `Added`, `Modified`, `Deleted`, `Renamed`, `Typechange`.
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    /// What this file's hunks say, as a key — the same job `content_key` does
    /// on a `git.status` row. A reader holding the row without the body uses
    /// it to tell a body it still holds from one that has moved on.
    pub content_key: String,
}

/// One changed path in a status walk: its staging tri-state, the key its body
/// caches under, and its line counts.
#[derive(Debug, Deserialize, Serialize)]
pub struct StatusFile {
    pub path: String,
    /// `all`, `partial`, or `none`.
    pub staged: String,
    pub index_status: String,
    pub worktree_status: String,
    pub content_key: String,
    /// Last modification, milliseconds since the epoch — what the SPA's
    /// relative "edited" label counts from. Absent for a path that no longer
    /// exists in the checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<u64>,
    pub added: u64,
    pub deleted: u64,
    pub binary: bool,
}

/// The full `git.status` answer — also what every mutating `git.*` verb
/// answers with, so the surface repaints from the mutation's own reply.
#[derive(Debug, Deserialize, Serialize)]
pub struct StatusPayload {
    pub branch: String,
    pub path: String,
    /// HEAD's commit id; absent on an unborn HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// `clean`, `merging`, `rebasing`, `cherry-picking`, `reverting`,
    /// `bisecting`, `conflicted`, or `other`. Documented as open.
    pub repo_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// Commits this branch holds that its upstream does not, and the other
    /// way round. Both are absent — the implementation writes `null` — when
    /// the branch tracks nothing: there is no count, and zero would read as
    /// "level with a remote" to a client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u64>,
    pub stash_count: u64,
    pub files: Vec<StatusFile>,
    pub files_truncated: bool,
    pub stat: DiffStat,
    pub status_key: String,
}

/// What `git.status` answers a client that already holds the working tree.
#[derive(Debug, Deserialize, Serialize)]
pub struct UnchangedStatus {
    pub unchanged: bool,
    pub status_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum GitStatusResult {
    /// `if_status_key` still named the working tree.
    Unchanged(UnchangedStatus),
    Fresh(Box<StatusPayload>),
}

/// One commit as `git.log` lists it and `git.show` leads with.
#[derive(Debug, Deserialize, Serialize)]
pub struct CommitSummary {
    pub short: String,
    pub hash: String,
    pub subject: String,
    pub author: String,
    pub email: String,
    /// Author time, seconds since the epoch.
    pub time: i64,
    /// Reachable from this checkout's HEAD but not from its base branch.
    /// Present on a run or worktree scope only; a project's history IS the
    /// base, so the field is omitted there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead_of_base: Option<bool>,
    /// Included by a workspace-directory history: this commit is part of the
    /// same unpublished range rendered by its All changes aggregate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpushed: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitLogResult {
    pub branch: String,
    pub commits: Vec<CommitSummary>,
    /// Another page follows.
    pub more: bool,
    /// This page cannot be prepended: the client replaces its log with it.
    ///
    /// Either the `since` cursor named no ancestor of HEAD, so these commits
    /// are the latest page rather than what landed after it, or the page ran
    /// out before it reached the cursor (`more` beside a cursored read) and
    /// prepending it would leave a hole the client could never ask for.
    #[serde(default)]
    pub reset: bool,
    /// HEAD, as the client's next `since`. Absent only on an unborn HEAD,
    /// which has no commit to cursor on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest: Option<String>,
    /// Names the base used for commit highlighting. A remote-tracking ref can
    /// move without HEAD changing, so clients use this to invalidate old pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight_key: Option<String>,
}

/// `git.show` — one commit's metadata, exact stat, and capped patch.
#[derive(Debug, Deserialize, Serialize)]
pub struct CommitDetail {
    #[serde(flatten)]
    pub summary: CommitSummary,
    pub body: String,
    pub stat: DiffStat,
    pub patch: String,
    /// How large the whole patch is, whether or not this answer carries all
    /// of it — what a client reads to decide whether opening it is worth a
    /// second call.
    #[serde(default)]
    pub patch_bytes: u64,
    /// The patch was cut: at the caller's `max_bytes` when it named one, at
    /// the 1 MiB wire cap otherwise. A patch cut at `max_bytes` carries the
    /// commit's file headers alone — which files moved, not how.
    pub truncated: bool,
    /// Where a ranged read's page sits in the whole patch. Only on an answer
    /// to a `range`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<BodySpan>,
}

/// One path's uncommitted patch, keyed so a client caches the body until the
/// file moves.
#[derive(Debug, Deserialize, Serialize)]
pub struct PatchFile {
    pub path: String,
    pub content_key: String,
    pub patch: String,
    pub truncated: bool,
    /// Where a ranged read's page sits in the whole patch. Only on an answer
    /// to a `range`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<BodySpan>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitDiffResult {
    pub files: Vec<PatchFile>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GitCommitResult {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub status: StatusPayload,
}

/// The checkout holding a branch, when one does.
#[derive(Debug, Deserialize, Serialize)]
pub struct BranchHolder {
    /// `run`, `project_repository`, or `external_worktree`.
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BranchRow {
    pub name: String,
    pub is_current: bool,
    /// The remote a remote-only branch came from; absent for a local branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub head_subject: String,
    pub head_time: i64,
    pub stat: DiffStat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<BranchHolder>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BranchListResult {
    pub current: String,
    pub branches: Vec<BranchRow>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FsListEntry {
    pub name: String,
    pub path: String,
    pub is_git: bool,
    pub is_hidden: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FsListResult {
    pub path: String,
    /// The directory to go up into; absent at the filesystem root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub is_git: bool,
    pub entries: Vec<FsListEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FsTreeEntry {
    pub name: String,
    /// `dir`, `file`, or `symlink`.
    pub kind: String,
    /// Files only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FsTreeResult {
    pub path: String,
    pub entries: Vec<FsTreeEntry>,
}

/// One file's bytes, as `fs.read` answers and `fs.write` answers back.
#[derive(Debug, Deserialize, Serialize)]
pub struct FsFileResult {
    pub path: String,
    pub size: u64,
    pub truncated: bool,
    pub mime: String,
    pub content_b64: String,
    /// Complete UTF-8 text a client may send back through `fs.write`.
    pub editable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    /// The exact bytes read, named — `fs.write` refuses a stale one. Absent
    /// when the read was truncated or asked for a range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Where a ranged read's page sits in the file, and which version of the
    /// file it was cut from. Only on an answer to a `range`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<BodySpan>,
}

/// A modification time per changed path that still exists in the checkout,
/// in milliseconds since the epoch (see [`StatusFile::edited_at`]).
pub type FileEditedAt = BTreeMap<String, u64>;

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectDiffResult {
    pub project_id: String,
    pub branch: String,
    pub path: String,
    pub stat: DiffStat,
    pub files: Vec<DiffFileRow>,
    /// Absent when the caller asked for the shape without it
    /// (`patch: false`): the rows and the key name a body the surface
    /// that opens it reads for itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

/// HEAD as the refs picker names it: the branch it is on, or the commit a
/// detached checkout sits at.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CurrentRef {
    /// Named first: only this variant carries `full_ref`.
    Branch(CurrentBranch),
    Detached(DetachedHead),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CurrentBranch {
    /// Always `branch`.
    pub kind: String,
    pub name: String,
    pub full_ref: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DetachedHead {
    /// Always `detached`.
    pub kind: String,
    pub commit: String,
}

/// One exact branch or tag `git.refs` offers, and where it stands against
/// what it tracks.
#[derive(Debug, Deserialize, Serialize)]
pub struct RefRow {
    /// `local`, `remote`, or `tag`.
    pub kind: String,
    pub name: String,
    pub full_ref: String,
    pub current: bool,
    /// The remote a remote-only branch came from. `null` for a local branch
    /// or a tag — the picker groups on the key being present.
    pub remote: Option<String>,
    /// The upstream a local branch tracks. `null` when it tracks nothing.
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
}

/// What `git.refs` answers: every checkoutable ref, and which one HEAD is.
#[derive(Debug, Deserialize, Serialize)]
pub struct RefListResult {
    pub current: CurrentRef,
    pub refs: Vec<RefRow>,
}

/// The commit `git.unpushed` measured against: the branch's push target when
/// it has one, the nearest published ancestor when it does not, or nothing at
/// all in a repository with no published history.
#[derive(Debug, Deserialize, Serialize)]
pub struct UnpushedBase {
    /// `push_target`, `published_ancestor`, or `empty`.
    pub kind: String,
    /// Named only by `push_target`; `null`, not absent, for the other two.
    pub label: Option<String>,
}

/// Everything this checkout has that its remote does not.
#[derive(Debug, Deserialize, Serialize)]
pub struct UnpushedDiff {
    /// Absent when the caller asked for the shape without it
    /// (`patch: false`): the rows and the key name a body the surface
    /// that opens it reads for itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    pub stat: DiffStat,
    pub files: Vec<DiffFileRow>,
    pub file_edited_at: FileEditedAt,
    pub diff_key: String,
    /// Whether the branch has a push target at all.
    pub published: bool,
    pub base: UnpushedBase,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum GitUnpushedResult {
    Unchanged(UnchangedDiff),
    Fresh(Box<UnpushedDiff>),
}

/// The folder `fs.mkdir` made, named the way the server sees it.
#[derive(Debug, Deserialize, Serialize)]
pub struct FsMkdirResult {
    pub path: String,
}

/// What a conditional diff read answers a client that already holds it.
#[derive(Debug, Deserialize, Serialize)]
pub struct UnchangedDiff {
    pub unchanged: bool,
    pub diff_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WorktreeDiff {
    pub worktree_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The branch this diff is anchored on.
    pub base_branch: String,
    pub head_subject: String,
    pub dirty_files: u64,
    pub path: String,
    /// The branch is the worktree's own, so Build could take it over.
    pub adoptable: bool,
    pub stat: DiffStat,
    pub files: Vec<DiffFileRow>,
    pub file_edited_at: FileEditedAt,
    /// Absent when the caller asked for the shape without it
    /// (`patch: false`): the rows and the key name a body the surface
    /// that opens it reads for itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    pub diff_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum WorktreeDiffResult {
    Unchanged(UnchangedDiff),
    Fresh(Box<WorktreeDiff>),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunDiff {
    pub stat: DiffStat,
    pub files: Vec<DiffFileRow>,
    /// Absent when the caller asked for the shape without it
    /// (`patch: false`): the rows and the key name a body the surface
    /// that opens it reads for itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    pub file_edited_at: FileEditedAt,
    pub diff_key: String,
    /// The issue that asked, when an issue surface did (`issue.diff`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<String>,
}

/// What `git.changeset_diff` answers: the asked paths' rows and their hunks,
/// under the whole changeset's key.
#[derive(Debug, Deserialize, Serialize)]
pub struct ChangesetDiff {
    pub stat: DiffStat,
    pub files: Vec<DiffFileRow>,
    pub patch: String,
    #[serde(default)]
    pub file_edited_at: FileEditedAt,
    /// Absent on a scope whose whole-patch verb answers no key either — the
    /// project's own checkout, whose surface reads it fresh every time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_key: Option<String>,
    /// Where a ranged read's page sits in the whole patch. Only on an answer
    /// to a `range`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<BodySpan>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ChangesetDiffResult {
    Unchanged(UnchangedDiff),
    Fresh(Box<ChangesetDiff>),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RunDiffResult {
    Unchanged(UnchangedDiff),
    Fresh(Box<RunDiff>),
}

/// A stage boundary that cannot be rendered: the two commits were never
/// pinned, or the stage has not completed.
#[derive(Debug, Deserialize, Serialize)]
pub struct StageDiffUnavailable {
    pub run_id: String,
    pub stage_id: String,
    /// Always `unavailable`.
    pub status: String,
    /// `legacy_unpinned` or `stage_not_complete`.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<String>,
}

/// One immutable stage boundary, sha to sha.
#[derive(Debug, Deserialize, Serialize)]
pub struct StageDiff {
    pub run_id: String,
    pub stage_id: String,
    /// Always `available`.
    pub status: String,
    pub start_sha: String,
    pub completion_sha: String,
    pub stat: DiffStat,
    pub files: Vec<DiffFileRow>,
    pub patch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum StageDiffResult {
    /// Named first: only this variant carries `reason`, and only the other
    /// carries `stat`, so neither can be read as the other.
    Unavailable(Box<StageDiffUnavailable>),
    Available(Box<StageDiff>),
}

// -------------------------------------------------------------- handlers ---

fn git_log(app: &mut AppState, params: GitLogParams) -> Result<Answer<GitLogResult>, ApiError> {
    answer(app.git_log(&params.wire()))
}

fn git_show(app: &mut AppState, params: GitShowParams) -> Result<Answer<CommitDetail>, ApiError> {
    answer(app.git_show(&params.wire()))
}

fn git_status(
    app: &mut AppState,
    params: GitStatusParams,
) -> Result<Answer<GitStatusResult>, ApiError> {
    answer(app.git_status(&params.wire()))
}

fn git_diff(app: &mut AppState, params: GitDiffParams) -> Result<Answer<GitDiffResult>, ApiError> {
    answer(app.git_diff(&params.wire()))
}

fn git_changeset_diff(
    app: &mut AppState,
    params: ChangesetDiffParams,
) -> Result<Answer<ChangesetDiffResult>, ApiError> {
    answer(app.changeset_diff(&params.wire()))
}

fn git_stage(
    app: &mut AppState,
    params: GitPathsParams,
) -> Result<Answer<StatusPayload>, ApiError> {
    answer(app.git_stage(&params.wire()))
}

fn git_unstage(
    app: &mut AppState,
    params: GitPathsParams,
) -> Result<Answer<StatusPayload>, ApiError> {
    answer(app.git_unstage(&params.wire()))
}

fn git_discard(
    app: &mut AppState,
    params: GitPathsParams,
) -> Result<Answer<StatusPayload>, ApiError> {
    answer(app.git_discard(&params.wire()))
}

fn git_commit(
    app: &mut AppState,
    params: GitCommitParams,
) -> Result<Answer<GitCommitResult>, ApiError> {
    answer(app.git_commit(&params.wire()))
}

fn git_fetch(app: &mut AppState, params: ScopeParams) -> Result<Answer<StatusPayload>, ApiError> {
    answer(app.git_fetch(&params.wire()))
}

fn git_pull(app: &mut AppState, params: GitPullParams) -> Result<Answer<StatusPayload>, ApiError> {
    answer(app.git_pull(&params.wire()))
}

fn git_push(app: &mut AppState, params: GitPushParams) -> Result<Answer<StatusPayload>, ApiError> {
    answer(app.git_push(&params.wire()))
}

fn git_stash(app: &mut AppState, params: ScopeParams) -> Result<Answer<StatusPayload>, ApiError> {
    answer(app.git_stash(&params.wire()))
}

fn git_stash_pop(
    app: &mut AppState,
    params: ScopeParams,
) -> Result<Answer<StatusPayload>, ApiError> {
    answer(app.git_stash_pop(&params.wire()))
}

fn git_merge_abort(
    app: &mut AppState,
    params: ScopeParams,
) -> Result<Answer<StatusPayload>, ApiError> {
    answer(app.git_merge_abort(&params.wire()))
}

fn git_branches(
    app: &mut AppState,
    params: BranchScopeParams,
) -> Result<Answer<BranchListResult>, ApiError> {
    answer(app.git_branches(&params.wire()))
}

fn git_checkout(
    app: &mut AppState,
    params: GitCheckoutParams,
) -> Result<Answer<StatusPayload>, ApiError> {
    answer(app.git_checkout(&params.wire()))
}

fn git_branch_delete(
    app: &mut AppState,
    params: GitBranchDeleteParams,
) -> Result<Answer<BranchListResult>, ApiError> {
    answer(app.git_branch_delete(&params.wire()))
}

fn git_refs(app: &mut AppState, params: ScopeParams) -> Result<Answer<RefListResult>, ApiError> {
    answer(app.git_refs(&params.wire()))
}

fn git_checkout_ref(
    app: &mut AppState,
    params: GitCheckoutRefParams,
) -> Result<Answer<StatusPayload>, ApiError> {
    answer(app.git_checkout_ref(&params.wire()))
}

fn git_unpushed(
    app: &mut AppState,
    params: GitUnpushedParams,
) -> Result<Answer<GitUnpushedResult>, ApiError> {
    answer(app.git_unpushed(&params.wire()))
}

fn fs_list(app: &mut AppState, params: FsListParams) -> Result<Answer<FsListResult>, ApiError> {
    answer(app.fs_list(&params.wire()))
}

fn fs_mkdir(app: &mut AppState, params: FsMkdirParams) -> Result<Answer<FsMkdirResult>, ApiError> {
    answer(app.fs_mkdir(&params.wire()))
}

fn fs_tree(app: &mut AppState, params: FsTreeParams) -> Result<Answer<FsTreeResult>, ApiError> {
    answer(app.fs_tree(&params.wire()))
}

fn fs_read(app: &mut AppState, params: FsReadParams) -> Result<Answer<FsFileResult>, ApiError> {
    answer(app.fs_read(&params.wire()))
}

fn fs_write(app: &mut AppState, params: FsWriteParams) -> Result<Answer<FsFileResult>, ApiError> {
    answer(app.fs_write(&params.wire()))
}

fn project_diff(
    app: &mut AppState,
    params: ProjectDiffParams,
) -> Result<Answer<ProjectDiffResult>, ApiError> {
    answer(app.project_diff(&params.wire()))
}

fn worktree_diff(
    app: &mut AppState,
    params: WorktreeDiffParams,
) -> Result<Answer<WorktreeDiffResult>, ApiError> {
    answer(app.worktree_diff(&params.wire()))
}

fn run_diff(app: &mut AppState, params: RunDiffParams) -> Result<Answer<RunDiffResult>, ApiError> {
    answer(app.run_diff(&params.wire()))
}

fn run_stage_diff(
    app: &mut AppState,
    params: RunStageDiffParams,
) -> Result<Answer<StageDiffResult>, ApiError> {
    answer(app.run_stage_diff(&params.wire()))
}

fn issue_diff(
    app: &mut AppState,
    params: IssueDiffParams,
) -> Result<Answer<RunDiffResult>, ApiError> {
    answer(app.issue_run_action(&params.wire(), "diff"))
}

fn issue_stage_diff(
    app: &mut AppState,
    params: IssueStageDiffParams,
) -> Result<Answer<StageDiffResult>, ApiError> {
    answer(app.issue_stage_diff(&params.wire()))
}

#[cfg(test)]
mod tests {
    use super::methods;
    use serde_json::json;

    /// The SPA's workspace directory scope names the directory's conversation
    /// beside the workspace pair, and a mutating verb's drain notes that
    /// entity changed (`entity_ids_of`). Every scoped verb the Changes and
    /// Files panes call takes it.
    #[test]
    fn a_workspace_directory_scope_may_name_its_conversation() {
        let scope = json!({
            "workspace_id": "ws-3f2a91c4",
            "source_id": "source-1",
            "entity_id": "run-7",
        });
        let calls = [
            ("git.status", json!({})),
            ("git.log", json!({ "limit": 20 })),
            ("git.show", json!({ "hash": "9f3c1a0b" })),
            ("git.unpushed", json!({ "patch": false })),
            ("git.changeset_diff", json!({ "paths": ["src/main.rs"] })),
            ("git.diff", json!({ "paths": ["src/main.rs"] })),
            ("git.stage", json!({ "paths": ["src/main.rs"] })),
            ("git.discard", json!({ "paths": ["src/main.rs"] })),
            ("git.commit", json!({ "message": "Fix" })),
            ("git.fetch", json!({})),
            ("git.pull", json!({ "mode": "ff" })),
            ("git.push", json!({})),
            ("git.stash", json!({})),
            ("git.stash_pop", json!({})),
            ("git.merge_abort", json!({})),
            ("git.refs", json!({})),
            ("git.checkout_ref", json!({ "full_ref": "refs/heads/main" })),
            ("fs.tree", json!({})),
            ("fs.read", json!({ "path": "README.md" })),
            (
                "fs.write",
                json!({ "path": "README.md", "expected_revision": "r1", "content_b64": "" }),
            ),
        ];
        for (method, own) in calls {
            let (_, handler) = methods()
                .iter()
                .find(|(name, _)| *name == method)
                .unwrap_or_else(|| panic!("{method} is not a git-family verb"));
            let mut params = scope.clone();
            params
                .as_object_mut()
                .unwrap()
                .extend(own.as_object().unwrap().clone());
            handler
                .parse_params(&params)
                .unwrap_or_else(|error| panic!("{method}: {error}"));
        }
    }
}
