use crate::app::{DeferredGit, DeferredWork, GitCallScope, ScopedGitCall};
use crate::lifecycle::holders::BranchHolder;
use crate::store::now_rfc3339;
pub(in crate::app) mod deferred;

use serde_json::{json, Value};

use crate::lifecycle::holders::ProjectCheckouts;

use super::{require_str, AppState, ReadSubject};

/// A resolved `git.*` scope: the repository directory the RPC operates on,
/// plus — for run scope — what `git.log` needs to mark commits ahead of base
/// and `git.commit` needs to invalidate afterwards.
pub(in crate::app) struct GitScope {
    pub(in crate::app) repo_path: std::path::PathBuf,
    /// Names browser cache entries for a source within a workspace. Legacy
    /// scopes keep their established keys; workspace directories need the
    /// enclosing id because different directories can contain identical Git
    /// state and identical relative paths.
    cache_namespace: Option<String>,
    /// Set for project scope: the project whose repository this is.
    pub(in crate::app) project_id: Option<String>,
    pub(in crate::app) run: Option<GitScopeRun>,
    pub(in crate::app) worktree: Option<GitScopeWorktree>,
}

pub(in crate::app) struct GitScopeRun {
    pub(in crate::app) run_id: String,
    pub(in crate::app) base_branch: String,
}

/// An external worktree's git scope: the project that owns it (so a mutation
/// invalidates the scan the rail reads) and the branch its history is measured
/// against.
pub(in crate::app) struct GitScopeWorktree {
    pub(in crate::app) project_id: String,
    pub(in crate::app) base_branch: String,
}

/// The checkout a branch operation acts on, and which cached summary describes
/// it: a project's repository, or one of that project's worktrees.
pub(in crate::app) struct BranchScope {
    pub(in crate::app) project_id: String,
    pub(in crate::app) repo_path: std::path::PathBuf,
    pub(in crate::app) base_branch: String,
    pub(in crate::app) external_worktree: bool,
}

/// A branch verb that answers with the project's branches: the checkout it
/// was scoped to, plus the checkouts the rows are stamped from.
pub(in crate::app) struct BranchListingScope {
    pub(in crate::app) checkout: BranchScope,
    pub(in crate::app) checkouts: ProjectCheckouts,
}

/// The answer `git.branches` and `git.branch_delete` share: gitgui's git facts
/// about every offerable branch, each row stamped with the checkout holding it.
/// Every held branch is published first, so a row weighs what its checkout
/// holds rather than what the project last saw of it.
pub(in crate::app) fn stamped_branch_list(scope: &BranchListingScope) -> Result<Value, String> {
    let ownership = scope.checkouts.holders()?;
    scope.checkouts.publish_held_branches(&ownership)?;
    let listing =
        crate::gitgui::branch_list(&scope.checkout.repo_path, &scope.checkout.base_branch)?;
    let branches: Vec<Value> = listing
        .rows
        .into_iter()
        .map(|row| {
            let holder = BranchHolder::of(&ownership, &row.name);
            let mut fields = row.into_json();
            fields["holder"] = holder.into_json();
            fields
        })
        .collect();
    Ok(json!({ "current": listing.current, "branches": branches }))
}

/// Parse the required `paths` param of `git.stage`/`git.unstage`: a non-empty
/// array of repo-relative strings.
pub(in crate::app) fn require_path_list(params: &Value) -> Result<Vec<String>, String> {
    let paths = params
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing required param: paths".to_string())?;
    if paths.is_empty() {
        return Err("paths must not be empty".to_string());
    }
    paths
        .iter()
        .map(|path| {
            path.as_str()
                .map(str::to_string)
                .ok_or_else(|| "paths must be an array of strings".to_string())
        })
        .collect()
}

/// The per-file rows a diff surface lists beside its patch.
pub(in crate::app) fn diff_file_rows(diff: &crate::diff::WorktreeDiff) -> Vec<Value> {
    diff.files()
        .iter()
        .map(|file| json!({ "path": file.path, "status": format!("{:?}", file.status) }))
        .collect()
}

impl GitScope {
    /// The meaning of highlighted commit rows in this scope. Workspace history
    /// marks the exact commits included by All changes; run/worktree history
    /// retains its base-branch marker; primary project history marks nothing.
    pub(in crate::app) fn log_highlight(&self) -> Option<crate::gitgui::LogHighlight<'_>> {
        if self.cache_namespace.is_some() {
            return Some(crate::gitgui::LogHighlight::Unpushed);
        }
        self.run
            .as_ref()
            .map(|run| crate::gitgui::LogHighlight::AheadOfBase(run.base_branch.as_str()))
            .or_else(|| {
                self.worktree
                    .as_ref()
                    .map(|wt| crate::gitgui::LogHighlight::AheadOfBase(wt.base_branch.as_str()))
            })
    }

    fn namespace_key(&self, key: &str) -> String {
        self.cache_namespace.as_ref().map_or_else(
            || key.to_string(),
            |namespace| crate::diff::fnv1a64_hex(&format!("{namespace}\0{key}")),
        )
    }

    fn namespace_payload_keys(&self, mut payload: Value) -> Value {
        if self.cache_namespace.is_none() {
            return payload;
        }
        if let Some(key) = payload
            .get("status_key")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            payload["status_key"] = json!(self.namespace_key(&key));
        }
        for file in payload["files"].as_array_mut().into_iter().flatten() {
            if let Some(key) = file
                .get("content_key")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                file["content_key"] = json!(self.namespace_key(&key));
            }
        }
        payload
    }

    fn status_payload(&self) -> Result<Value, String> {
        crate::gitgui::status_payload(&self.repo_path)
            .map(|payload| self.namespace_payload_keys(payload))
    }

    fn status_payload_unless(&self, held_key: Option<&str>) -> Result<Value, String> {
        if self.cache_namespace.is_none() {
            return crate::gitgui::status_payload_unless(&self.repo_path, held_key);
        }
        let payload = self.status_payload()?;
        let current_key = payload["status_key"].as_str().unwrap_or_default();
        if held_key == Some(current_key) {
            Ok(json!({ "unchanged": true, "status_key": current_key }))
        } else {
            Ok(payload)
        }
    }

    fn file_patches(&self, paths: &[String]) -> Result<Value, String> {
        crate::gitgui::file_patches(&self.repo_path, paths)
            .map(|payload| self.namespace_payload_keys(payload))
    }
}

impl AppState {
    /// The primary checkout's uncommitted-changes review surface (spec §5.2):
    /// same shape as `worktree.diff` so `parseDiff`/`diffFilesHtml` reuse is
    /// mechanical.
    pub(crate) fn project_diff(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let repo_path = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|project| {
                if project.is_git {
                    Ok(project.repo_path.clone())
                } else {
                    Err("project is not a git repository; initialize Git first".to_string())
                }
            })
            .ok_or_else(|| "unknown project_id".to_string())?;
        let repo_path = repo_path?;
        Ok(self.defer_read(
            ReadSubject::Project {
                project_id,
                repo_path,
            },
            None,
        ))
    }

    /// Read-only browse of one external worktree's dirty diff (spec §5.4) —
    /// never adopts.
    pub(crate) fn worktree_diff(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let worktree_id = require_str(params, "worktree_id")?;
        let external = self.resolve_external_worktree(&project_id, &worktree_id)?;
        let base_branch = self.base_for(&project_id)?;
        Ok(self.defer_conditional_read(
            ReadSubject::Worktree {
                external: Box::new(external),
                base_branch,
            },
            None,
            params.get("if_diff_key").and_then(Value::as_str),
            crate::app::wants_patch(params),
        ))
    }

    fn resolve_workspace_git_scope(&mut self, params: &Value) -> Result<Option<GitScope>, String> {
        let workspace_id = optional_scope_id(params, "workspace_id")?;
        let source_id = optional_scope_id(params, "source_id")?;
        if workspace_id.is_none() && source_id.is_none() {
            return Ok(None);
        }
        let workspace_id =
            workspace_id.ok_or_else(|| "missing required param: workspace_id".to_string())?;
        let source_id = source_id.ok_or_else(|| "missing required param: source_id".to_string())?;
        if ["project_id", "run_id", "worktree_id", "task_id"]
            .iter()
            .any(|field| params.get(field).is_some())
        {
            return Err(
                "workspace source scope cannot be combined with a legacy git scope".to_string(),
            );
        }
        let directory = self.resolve_workspace_directory(&workspace_id, &source_id)?;
        if !directory.is_git {
            return Err("workspace source is not a git repository".to_string());
        }
        Ok(Some(GitScope {
            repo_path: directory.path,
            cache_namespace: Some(format!(
                "workspace:{workspace_id}:directory:{}",
                directory.id
            )),
            project_id: None,
            run: None,
            worktree: None,
        }))
    }

    /// Resolve the shared `git.*` scope: one source in a workspace
    /// (`workspace_id` + `source_id`), the project's primary checkout
    /// (`project_id` alone), a run's worktree (`run_id`), or one of the
    /// project's external worktrees (`project_id` + `worktree_id`). The repo
    /// path always comes from server state — a client can never name a
    /// filesystem path directly.
    pub(in crate::app) fn resolve_git_scope(&mut self, params: &Value) -> Result<GitScope, String> {
        if let Some(scope) = self.resolve_workspace_git_scope(params)? {
            return Ok(scope);
        }
        let project_id = params
            .get("project_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let run_id = params
            .get("run_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let worktree_id = params
            .get("worktree_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        match (project_id, run_id, worktree_id) {
            // A worktree is named within its project, so both ids arrive
            // together — and the worktree, being the narrower of the two, is
            // what the RPC operates on.
            (Some(project_id), None, Some(worktree_id)) => {
                let external = self.resolve_external_worktree(&project_id, &worktree_id)?;
                let base_branch = self.base_for(&project_id)?;
                Ok(GitScope {
                    repo_path: external.path,
                    cache_namespace: None,
                    project_id: None,
                    run: None,
                    worktree: Some(GitScopeWorktree {
                        project_id,
                        base_branch,
                    }),
                })
            }
            (Some(project_id), None, None) => {
                let project = self
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .ok_or_else(|| "unknown project_id".to_string())?;
                if !project.is_git {
                    return Err("project is not a git repository; initialize Git first".to_string());
                }
                Ok(GitScope {
                    repo_path: project.repo_path.clone(),
                    cache_namespace: None,
                    project_id: Some(project.id.clone()),
                    run: None,
                    worktree: None,
                })
            }
            (None, Some(run_id), None) => {
                let active = self
                    .runs
                    .get(&run_id)
                    .ok_or_else(|| "unknown run_id".to_string())?;
                let repo_path = self.run_git_root(&run_id, &active.worktree.path);
                let base_branch = self.run_base_branch(&run_id, &active.worktree.base_branch);
                Ok(GitScope {
                    repo_path,
                    cache_namespace: None,
                    project_id: None,
                    run: Some(GitScopeRun {
                        run_id,
                        base_branch,
                    }),
                    worktree: None,
                })
            }
            _ => Err(
                "provide exactly one of project_id, run_id, or project_id + worktree_id"
                    .to_string(),
            ),
        }
    }

    /// Resolve a `git.*` verb's checkout under the lock and hand the git call
    /// itself to the drain, which makes it with the mutex released.
    ///
    /// `invalidates` says a successful call leaves the board's cached
    /// summaries describing a tree that has since changed.
    pub(in crate::app) fn defer_git(
        &mut self,
        params: &Value,
        invalidates: bool,
        work: fn(&GitScope, &Value) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        Ok(self.defer_git_work(scope, work, params, invalidates))
    }

    /// [`AppState::defer_git`] for the verbs that address the repository's
    /// branches rather than one checkout's working tree.
    pub(in crate::app) fn defer_branch_git(
        &mut self,
        params: &Value,
        invalidates: bool,
        work: fn(&BranchScope, &Value) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let scope = self.resolve_branch_scope(params)?;
        Ok(self.defer_git_work(scope, work, params, invalidates))
    }

    /// [`AppState::defer_branch_git`] for the verbs that render the project's
    /// branches. They alone carry [`ProjectCheckouts`], which a checkout verb
    /// has no row to stamp with; the drain asks git who holds what.
    pub(in crate::app) fn defer_branch_listing(
        &mut self,
        params: &Value,
        invalidates: bool,
        work: fn(&BranchListingScope, &Value) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let checkout = self.resolve_branch_scope(params)?;
        let checkouts = self.project_checkouts(&checkout.project_id)?;
        let scope = BranchListingScope {
            checkout,
            checkouts,
        };
        Ok(self.defer_git_work(scope, work, params, invalidates))
    }

    pub(in crate::app) fn defer_git_work<S: GitCallScope + 'static>(
        &mut self,
        scope: S,
        work: fn(&S, &Value) -> Result<Value, String>,
        params: &Value,
        invalidates: bool,
    ) -> Value {
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(ScopedGitCall { scope, work }),
            params: params.clone(),
            invalidates,
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        })));
        Value::Null
    }

    /// Write back a git verb that ran with the mutex released: drop the cached
    /// summaries its mutation made stale.
    ///
    /// STALENESS: the checkout may have left the board while the git ran (its
    /// run released, finished, or deleted). The answer still stands — it
    /// describes what the tree did — but the cache write is dropped rather
    /// than stamping an entity that is gone back into the daemon's maps.
    pub(in crate::app) fn apply_git(
        &mut self,
        git: &DeferredGit,
        result: Result<Value, String>,
    ) -> Result<Value, String> {
        let result = result.and_then(|value| git.call.settle(self, value));
        if !git.invalidates || (result.is_err() && !git.call.invalidates_on_error()) {
            return result;
        }
        if result.is_ok() && git.params.get("source_id").is_some() {
            if let Some(workspace_id) = git.params.get("workspace_id").and_then(Value::as_str) {
                self.reopen_workspace(workspace_id)?;
            }
        }
        git.call.invalidate(self);
        result
    }

    /// A branch switch swaps the whole tree, so whichever summary described
    /// the scoped checkout is stale. The project's own checkout has none: the
    /// board lists workspaces, and nothing on it is read off the repository.
    pub(in crate::app) fn invalidate_branch_scope_caches(&mut self, scope: &BranchScope) {
        if !self.projects.iter().any(|p| p.id == scope.project_id) {
            return;
        }
        if scope.external_worktree {
            self.rescan_external_worktrees(&scope.project_id);
        }
    }

    /// Whether the entity a git scope spoke for is still on the board.
    pub(in crate::app) fn git_scope_is_current(&self, scope: &GitScope) -> bool {
        if let Some(run) = &scope.run {
            return self.runs.contains_key(&run.run_id);
        }
        let project_id = scope
            .project_id
            .as_deref()
            .or(scope.worktree.as_ref().map(|w| w.project_id.as_str()));
        project_id.is_some_and(|project_id| self.projects.iter().any(|p| p.id == project_id))
    }

    /// `git.refs` — every exact local branch, cached remote branch, and tag offered
    /// by a workspace directory, including its current branch or detached commit.
    /// The shared resolver keeps this available to the legacy scopes too.
    pub(crate) fn git_refs(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, _| {
            Ok(crate::gitgui::ref_list(&scope.repo_path)?.into_json())
        })
    }

    /// `git.checkout_ref` — check out one exact listed branch or tag. Git's
    /// overwrite refusal is returned unchanged, leaving the checkout and its
    /// local changes in place.
    pub(crate) fn git_checkout_ref(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, params| {
            let full_ref = require_str(params, "full_ref")?;
            crate::gitgui::checkout_ref(&scope.repo_path, &full_ref)?;
            scope.status_payload()
        })
    }

    /// `git.log` — one page of commit history for the scoped checkout. Task
    /// scope additionally marks each commit as ahead of (unreachable from)
    /// the base branch.
    pub(crate) fn git_log(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, params| {
            let since = params.get("since").and_then(Value::as_str);
            // A cursored read asks what has landed since a hash it already
            // holds, and the answer to that is usually nothing — so its
            // default page is the small one. A browsing read keeps the page
            // it has always had.
            let default_limit = match since {
                Some(_) => crate::api::v1::git::LATEST_COMMITS,
                None => 30,
            };
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(default_limit)
                .clamp(1, 200) as usize;
            let skip = params.get("skip").and_then(Value::as_u64).unwrap_or(0) as usize;
            crate::gitgui::log_page(&scope.repo_path, scope.log_highlight(), limit, skip, since)
        })
    }

    /// `git.show` — one commit's metadata, stat, and capped patch. The hash
    /// param is a strict object-id prefix, never a general revspec.
    pub(crate) fn git_show(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, params| {
            let hash = require_str(params, "hash")?;
            // Clamped, never refused: a caller that asked for a byte wants a
            // small answer, and one that asked for the moon wants the most
            // the wire carries.
            let max_bytes = params
                .get("max_bytes")
                .and_then(Value::as_u64)
                .map(|cap| cap.clamp(1024, crate::api::v1::git::COMMIT_PATCH_MAX_BYTES) as usize);
            crate::gitgui::show_commit(&scope.repo_path, &hash, max_bytes)
        })
    }

    /// `git.status` — branch/head, per-file staging tri-state, content keys and
    /// line counts for the scoped checkout. No patch: a file's body comes from
    /// `git.diff`, per path.
    ///
    /// `if_status_key` is what the browser is already painting; when it still
    /// names the working tree the answer is `{"unchanged": true}` and its key.
    pub(crate) fn git_status(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, params| {
            let if_status_key = params.get("if_status_key").and_then(Value::as_str);
            scope.status_payload_unless(if_status_key)
        })
    }

    /// Every local change not represented by the checkout's push destination.
    pub(crate) fn git_unpushed(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, params| {
            crate::gitgui::unpushed_payload(
                &scope.repo_path,
                params.get("if_diff_key").and_then(Value::as_str),
                // Absent means yes: a client that has not heard of the flag
                // is answered exactly as it always was.
                params.get("patch").and_then(Value::as_bool).unwrap_or(true),
            )
        })
    }

    /// `git.diff` — the uncommitted patch of the named paths, one entry each,
    /// keyed by content so a browser caches a body until that file moves.
    pub(crate) fn git_diff(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, params| {
            let paths = require_path_list(params)?;
            scope.file_patches(&paths)
        })
    }

    /// `git.stage` — stage the given repo-relative paths, answering with the
    /// fresh status payload so the UI repaints without waiting for a poll.
    pub(crate) fn git_stage(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, params| {
            let paths = require_path_list(params)?;
            crate::gitgui::stage_paths(&scope.repo_path, &paths)?;
            scope.status_payload()
        })
    }

    /// `git.unstage` — the inverse of `git.stage`, same response shape.
    pub(crate) fn git_unstage(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, params| {
            let paths = require_path_list(params)?;
            crate::gitgui::unstage_paths(&scope.repo_path, &paths)?;
            scope.status_payload()
        })
    }

    /// `git.commit` — commit exactly what is staged with the user's message.
    /// On a task scope the commit changes the tree the board summarizes, so
    /// the cached diffstat is dropped and the task's updated-at stamped; the
    /// task record itself is untouched (no lifecycle transition — a commit
    /// never advances a task past any gate).
    pub(crate) fn git_commit(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, params| {
            let message = require_str(params, "message")?;
            let commit = crate::gitgui::commit_staged(&scope.repo_path, &message)?;
            let status = scope.status_payload()?;
            Ok(json!({
                "hash": commit["hash"],
                "short": commit["short"],
                "subject": commit["subject"],
                "status": status,
            }))
        })
    }

    /// Drop the cached board summaries a scoped git mutation just invalidated —
    /// the task's diffstat + updated-at for task scope, the external-worktree
    /// scan for worktree scope — so the next `task.list` / project poll
    /// recomputes instead of serving a stale summary for up to its TTL.
    pub(in crate::app) fn invalidate_git_scope_caches(&mut self, scope: &GitScope) {
        if let Some(run) = &scope.run {
            let run_id = run.run_id.clone();
            self.invalidate_run_stat(&run_id);
            self.board
                .attention_mut()
                .record_updated(&run_id, now_rfc3339());
        }
        if let Some(worktree) = &scope.worktree {
            let project_id = worktree.project_id.clone();
            self.rescan_external_worktrees(&project_id);
        }
    }

    /// Resolve the checkout a branch operation acts on: the project's
    /// repository, or one of its external worktrees when the caller names one. A
    /// run worktree's branch is owned by the run lifecycle, so a `run_id` (or
    /// its legacy `task_id` spelling) is refused outright.
    pub(in crate::app) fn resolve_branch_scope(
        &mut self,
        params: &Value,
    ) -> Result<BranchScope, String> {
        if params.get("task_id").is_some() || params.get("run_id").is_some() {
            return Err("branch operations are project- or worktree-scope only".to_string());
        }
        let project_id = require_str(params, "project_id")?;
        let project = self.project_for(&project_id)?;
        let base_branch = project.base_branch.clone();
        let primary_repo_path = project.repo_path.clone();
        let (repo_path, external_worktree) = match params.get("worktree_id").and_then(Value::as_str)
        {
            Some(worktree_id) => (
                self.resolve_external_worktree(&project_id, worktree_id)?
                    .path,
                true,
            ),
            None => (primary_repo_path, false),
        };
        Ok(BranchScope {
            project_id,
            repo_path,
            base_branch,
            external_worktree,
        })
    }

    /// `git.fetch` — `git fetch --prune`, then the fresh status payload.
    pub(crate) fn git_fetch(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, _| {
            crate::gitgui::fetch(&scope.repo_path)?;
            scope.status_payload()
        })
    }

    /// `git.pull` — integrate the upstream in the requested mode (ff/merge/
    /// rebase), then the fresh status payload. Git's own errors pass through.
    pub(crate) fn git_pull(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, params| {
            let mode = params.get("mode").and_then(Value::as_str).unwrap_or("ff");
            crate::gitgui::pull(&scope.repo_path, mode)?;
            scope.status_payload()
        })
    }

    /// `git.push` — push the current branch (setting the upstream on first
    /// push), then the fresh status payload. `force` uses `--force-with-lease`.
    pub(crate) fn git_push(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, params| {
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            crate::gitgui::push(&scope.repo_path, force)?;
            scope.status_payload()
        })
    }

    /// `git.branches` — every branch the project can offer, once each (the same
    /// list whichever checkout is scoped: branches are the repository's, not
    /// one checkout's), each stamped with the checkout that holds it.
    pub(crate) fn git_branches(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_branch_listing(params, false, |scope, _| stamped_branch_list(scope))
    }

    /// `git.checkout` — switch the scoped checkout to (or create) a branch,
    /// then the fresh status payload.
    pub(crate) fn git_checkout(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_branch_git(params, true, |scope, params| {
            let branch = require_str(params, "branch")?;
            let create = params
                .get("create")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            crate::gitgui::checkout(&scope.repo_path, &branch, create)?;
            crate::gitgui::status_payload(&scope.repo_path)
        })
    }

    /// `git.branch_delete` — delete a local branch, then the fresh branch list.
    pub(crate) fn git_branch_delete(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_branch_listing(params, false, |scope, params| {
            let branch = require_str(params, "branch")?;
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            crate::gitgui::branch_delete(&scope.checkout.repo_path, &branch, force)?;
            stamped_branch_list(scope)
        })
    }

    /// `git.stash` — `git stash push -u`, then the fresh status payload.
    pub(crate) fn git_stash(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, _| {
            crate::gitgui::stash_push(&scope.repo_path)?;
            scope.status_payload()
        })
    }

    /// `git.stash_pop` — `git stash pop`, then the fresh status payload.
    pub(crate) fn git_stash_pop(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, _| {
            crate::gitgui::stash_pop(&scope.repo_path)?;
            scope.status_payload()
        })
    }

    /// `git.discard` (**destructive**) — revert the given paths to HEAD
    /// (untracked ones are deleted), then the fresh status payload.
    pub(crate) fn git_discard(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, params| {
            let paths = require_path_list(params)?;
            crate::gitgui::discard_paths(&scope.repo_path, &paths)?;
            scope.status_payload()
        })
    }

    /// `git.merge_abort` — abort whatever operation is in progress (merge,
    /// rebase, cherry-pick, revert, or bisect), then the fresh status payload.
    pub(crate) fn git_merge_abort(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, _| {
            crate::gitgui::merge_abort(&scope.repo_path)?;
            scope.status_payload()
        })
    }
}

/// Parse an optional scope id without letting malformed workspace fields fall
/// through to one of the legacy scope shapes.
fn optional_scope_id(params: &Value, name: &str) -> Result<Option<String>, String> {
    match params.get(name) {
        None => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Err(format!("{name} cannot be empty")),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{name} must be a string")),
    }
}
