use super::*;
use crate::store::now_rfc3339;
use crate::store::reviews::{load_review, snapshot_exists, write_header};
use crate::store::tracker::{append_activity, next_task_number, write_tracker_task};
use crate::tracker::{Task, TaskEvent, IN_REVIEW_STATUS};
use std::collections::{BTreeMap, BTreeSet};

impl Store {
    /// Reserve identity and number before any Git mutation. Same request/args
    /// resumes its original record, including after publication or cancellation.
    pub fn reserve_review_opening(
        &self,
        project_path: &str,
        request_id: &str,
        request: ReviewOpeningRequest,
    ) -> Result<ReviewOpening, StoreError> {
        self.in_transaction(|tx| {
            if let Some(existing) = load_opening(tx, project_path, request_id)? {
                if existing.request != request {
                    return Err(StoreError::ReviewRequestConflict {
                        request_id: request_id.into(),
                    });
                }
                return Ok(existing);
            }
            validate_request(request_id, &request)?;
            let now = now_rfc3339();
            let mut task =
                Task::drafted(project_path, &request.title, request.creator.clone(), &now);
            task.number = next_task_number(tx, project_path)?;
            task.body = request.description.clone();
            task.status = IN_REVIEW_STATUS.into();
            task.assignee = request.reviewer.clone();
            task.links.workspace_ids.push(request.workspace_id.clone());
            claim_workspace(tx, project_path, &request.workspace_id, &task.id)?;
            let opening = ReviewOpening {
                project_path: project_path.into(),
                request_id: request_id.into(),
                version: 1,
                request,
                task,
                state: ReviewOpeningState::Preparing,
                bindings: Vec::new(),
                created_at: now.clone(),
                updated_at: now,
                error: None,
            };
            write_opening(tx, &opening)?;
            Ok(opening)
        })
    }

    pub fn load_review_opening(
        &self,
        project_path: &str,
        request_id: &str,
    ) -> Result<Option<ReviewOpening>, StoreError> {
        load_opening(&self.connection(), project_path, request_id)
    }

    /// Preparation survives restarts without requiring a client to remember
    /// its request ID. Store opens do not perform or replay recovery themselves.
    pub fn load_unfinished_review_openings(&self) -> Result<Vec<ReviewOpening>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT project_key, request_id, record FROM review_openings WHERE state IN ('preparing', 'failed', 'interrupted') ORDER BY project_key, number")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (project, request, raw) = row?;
            decode(&raw, "review_openings", &format!("{project}/{request}"))
        })
        .collect()
    }

    /// Save preparation/recovery progress with immutable request/task/binding
    /// identity. Cancel only after the service has verified safe Git unwind.
    pub fn save_review_opening(
        &self,
        opening: &ReviewOpening,
        expected_version: u64,
    ) -> Result<ReviewOpening, StoreError> {
        self.in_transaction(|tx| {
            let previous = load_opening(tx, &opening.project_path, &opening.request_id)?
                .ok_or_else(|| invalid("unknown opening request"))?;
            check_operation_version(&opening.request_id, expected_version, previous.version)?;
            validate_opening_update(&previous, opening)?;
            if opening.state.is_claiming_workspace() {
                claim_workspace(
                    tx,
                    &opening.project_path,
                    &opening.request.workspace_id,
                    &opening.task.id,
                )?;
                write_bindings(tx, opening)?;
            } else {
                release_workspace(tx, &opening.task.id)?;
                tx.execute(
                    "DELETE FROM review_branch_bindings WHERE task_id = ?1",
                    [&opening.task.id],
                )?;
            }
            let mut saved = opening.clone();
            saved.version = previous.version + 1;
            saved.updated_at = now_rfc3339();
            write_opening(tx, &saved)?;
            Ok(saved)
        })
    }

    /// Publish the reserved task, fixed membership/bindings and first pinned
    /// snapshot in one commit. Git pins must exist before calling this method.
    pub fn publish_review_opening(
        &self,
        project_path: &str,
        request_id: &str,
        expected_version: u64,
        mut snapshot: ReviewSnapshot,
        events: &[TaskEvent],
    ) -> Result<crate::reviews::records::Review, StoreError> {
        self.in_transaction(|tx| {
            let mut opening = load_opening(tx, project_path, request_id)?
                .ok_or_else(|| invalid("unknown opening request"))?;
            if opening.state == ReviewOpeningState::Published {
                return load_review(tx, &opening.task.id)?
                    .ok_or_else(|| invalid("published opening has no review"));
            }
            check_operation_version(request_id, expected_version, opening.version)?;
            validate_publication(tx, &opening, &snapshot, events)?;
            snapshot.number = 1;
            for binding in &opening.bindings {
                crate::tracker::TaskLinks::add(
                    &mut opening.task.links.branches,
                    binding
                        .dedicated_branch_ref
                        .strip_prefix("refs/heads/")
                        .expect("branch ref validated"),
                );
            }
            write_tracker_task(tx, &opening.task)?;
            let header = ReviewHeader {
                task_id: opening.task.id.clone(),
                workspace_id: opening.request.workspace_id.clone(),
                version: 1,
                state: crate::reviews::records::ReviewState::Open,
                completion: None,
                mode: ReviewMode::PullRequest,
                pull_request: Some(PullRequestMetadata {
                    status: PullRequestStatus::Open,
                    creator: opening.request.creator.clone(),
                    originating_workspace_id: opening.request.workspace_id.clone(),
                    latest_published_snapshot_id: Some(snapshot.id.clone()),
                    directories: opening.request.directories.clone(),
                }),
            };
            write_header(tx, &header)?;
            write_bindings(tx, &opening)?;
            tx.execute(
                "INSERT INTO review_snapshots (id, task_id, number, record) VALUES (?1, ?2, 1, ?3)",
                params![
                    snapshot.id,
                    opening.task.id,
                    serde_json::to_string(&snapshot).expect("snapshot serializes")
                ],
            )?;
            append_activity(tx, &[], events)?;
            opening.version += 1;
            opening.state = ReviewOpeningState::Published;
            opening.updated_at = now_rfc3339();
            opening.error = None;
            write_opening(tx, &opening)?;
            Ok(load_review(tx, &opening.task.id)?.expect("PR was published"))
        })
    }
}

fn validate_request(id: &str, request: &ReviewOpeningRequest) -> Result<(), StoreError> {
    if id.is_empty() || request.workspace_id.is_empty() || request.title.trim().is_empty() {
        return Err(invalid("opening needs a request ID, workspace and title"));
    }
    let mut ids = BTreeSet::new();
    for member in &request.directories {
        if member.directory_id.is_empty()
            || member.source_id.is_empty()
            || !ids.insert(&member.directory_id)
        {
            return Err(invalid(
                "directory membership must have unique, nonempty identities",
            ));
        }
    }
    let git_ids: BTreeSet<_> = request
        .directories
        .iter()
        .filter(|member| member.kind == ReviewMembershipKind::Git)
        .map(|member| &member.directory_id)
        .collect();
    if git_ids.is_empty()
        || git_ids != request.base_branches.keys().collect()
        || request
            .base_branches
            .values()
            .any(|base| !base.starts_with("refs/heads/") || base.len() == "refs/heads/".len())
    {
        return Err(invalid(
            "every included Git directory needs an explicit full base ref",
        ));
    }
    Ok(())
}

fn validate_opening_update(
    previous: &ReviewOpening,
    next: &ReviewOpening,
) -> Result<(), StoreError> {
    if previous.request != next.request
        || previous.task != next.task
        || previous.created_at != next.created_at
        || previous.project_path != next.project_path
        || previous.request_id != next.request_id
    {
        return Err(invalid("opening identity and reserved task cannot change"));
    }
    if matches!(
        previous.state,
        ReviewOpeningState::Published | ReviewOpeningState::Cancelled
    ) || next.state == ReviewOpeningState::Published
    {
        return Err(invalid(
            "terminal opening journal cannot be changed; use publication to publish",
        ));
    }
    validate_planned_bindings(&next.request, &next.bindings)?;
    if !previous.bindings.is_empty() {
        let old: BTreeMap<_, _> = previous
            .bindings
            .iter()
            .map(|binding| (&binding.directory_id, binding_identity(binding)))
            .collect();
        let new: BTreeMap<_, _> = next
            .bindings
            .iter()
            .map(|binding| (&binding.directory_id, binding_identity(binding)))
            .collect();
        if old != new {
            return Err(invalid(
                "planned branch/repository identities cannot change",
            ));
        }
    }
    Ok(())
}

fn validate_planned_bindings(
    request: &ReviewOpeningRequest,
    bindings: &[ReviewBranchBinding],
) -> Result<(), StoreError> {
    let count = request
        .directories
        .iter()
        .filter(|member| member.kind == ReviewMembershipKind::Git)
        .count();
    if !bindings.is_empty() && bindings.len() != count {
        return Err(invalid(
            "journal the complete branch plan before preparing any directory",
        ));
    }
    let mut ids = BTreeSet::new();
    for binding in bindings {
        if !ids.insert(&binding.directory_id) {
            return Err(invalid("duplicate directory binding"));
        }
        validate_binding(request, binding)?;
    }
    Ok(())
}

fn validate_binding(
    request: &ReviewOpeningRequest,
    binding: &ReviewBranchBinding,
) -> Result<(), StoreError> {
    let member = request.directories.iter().find(|member| {
        member.directory_id == binding.directory_id
            && member.source_id == binding.source_id
            && member.kind == ReviewMembershipKind::Git
    });
    if member.is_none()
        || request.base_branches.get(&binding.directory_id) != Some(&binding.base_branch_ref)
    {
        return Err(invalid(
            "binding must match fixed membership and selected base",
        ));
    }
    if binding.repository_id.is_empty()
        || binding.remote_name.is_empty()
        || binding.initial_head.is_empty()
        || !binding.working_repository.is_absolute()
        || !binding.source_repository.is_absolute()
        || !binding.receiving_repository.is_absolute()
    {
        return Err(invalid(
            "bindings need repository identities, an initial HEAD and absolute paths",
        ));
    }
    let refs = [
        Some(&binding.dedicated_branch_ref),
        Some(&binding.base_branch_ref),
        binding.original_branch_ref.as_ref(),
    ];
    if refs
        .into_iter()
        .flatten()
        .any(|name| !git2::Reference::is_valid_name(name) || !name.starts_with("refs/heads/"))
    {
        return Err(invalid("branch bindings must store valid full branch refs"));
    }
    if !git2::Reference::is_valid_name(&binding.receiving_ref)
        || !binding.receiving_ref.starts_with("refs/")
    {
        return Err(invalid("receiving refs must be valid full refs"));
    }
    Ok(())
}

fn validate_publication(
    tx: &Transaction,
    opening: &ReviewOpening,
    snapshot: &ReviewSnapshot,
    events: &[TaskEvent],
) -> Result<(), StoreError> {
    if opening.state != ReviewOpeningState::Preparing {
        return Err(invalid("opening must finish recovery before publication"));
    }
    if load_header(tx, &opening.task.id)?.is_some()
        || tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM tracker_tasks WHERE id = ?1)",
            [&opening.task.id],
            |row| row.get::<_, bool>(0),
        )?
    {
        return Err(invalid("reserved task identity is already published"));
    }
    if snapshot.id.is_empty() || snapshot_exists(tx, &snapshot.id)? {
        return Err(StoreError::ReviewSnapshotExists {
            snapshot_id: snapshot.id.clone(),
        });
    }
    if events.iter().any(|event| event.task_id != opening.task.id) {
        return Err(invalid("opening activity must belong to the reserved task"));
    }
    validate_planned_bindings(&opening.request, &opening.bindings)?;
    validate_snapshot(opening, snapshot)
}

fn validate_snapshot(opening: &ReviewOpening, snapshot: &ReviewSnapshot) -> Result<(), StoreError> {
    let expected: BTreeSet<_> = opening
        .request
        .directories
        .iter()
        .map(|member| (&member.directory_id, &member.source_id))
        .collect();
    let found: BTreeSet<_> = snapshot
        .directories
        .iter()
        .map(|directory| (&directory.id, &directory.source_id))
        .collect();
    if expected != found || found.len() != snapshot.directories.len() {
        return Err(invalid(
            "snapshot must preserve the complete fixed directory membership",
        ));
    }
    let git_count = opening
        .request
        .directories
        .iter()
        .filter(|member| member.kind == ReviewMembershipKind::Git)
        .count();
    if opening.bindings.len() != git_count {
        return Err(invalid(
            "every included Git directory needs a prepared binding",
        ));
    }
    for binding in &opening.bindings {
        let directory = snapshot
            .directories
            .iter()
            .find(|directory| directory.id == binding.directory_id)
            .expect("membership checked");
        validate_published_directory(binding, directory)?;
    }
    Ok(())
}

fn validate_published_directory(
    binding: &ReviewBranchBinding,
    directory: &ReviewDirectory,
) -> Result<(), StoreError> {
    if binding.preparation != ReviewPreparationState::Ready
        || binding.publication != ReviewPublicationState::Published
        || binding.recovery.is_some()
        || binding.last_received_head.is_none()
    {
        return Err(invalid(
            "publication requires every binding to be prepared and received",
        ));
    }
    if directory.status != ReviewDirectoryStatus::Git
        || !directory.is_git
        || directory.head != binding.last_received_head
        || directory.common_git_dir.as_ref() != Some(&binding.receiving_repository)
        || directory.base.is_none()
    {
        return Err(invalid(
            "snapshot must capture the bound receiver head and comparison base",
        ));
    }
    Ok(())
}

fn write_bindings(tx: &Transaction, opening: &ReviewOpening) -> Result<(), StoreError> {
    for binding in &opening.bindings {
        tx.execute("INSERT INTO review_branch_bindings (task_id, directory_id, repository_id, dedicated_ref, receiving_repository, receiving_ref, record) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(task_id, directory_id) DO UPDATE SET record = ?7", params![opening.task.id, binding.directory_id, binding.repository_id, binding.dedicated_branch_ref, binding.receiving_repository.to_string_lossy(), binding.receiving_ref, serde_json::to_string(binding).expect("binding serializes")])?;
    }
    Ok(())
}

fn load_opening(
    conn: &Connection,
    project: &str,
    request: &str,
) -> Result<Option<ReviewOpening>, StoreError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT record FROM review_openings WHERE project_key = ?1 AND request_id = ?2",
            params![project, request],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| decode(&raw, "review_openings", &format!("{project}/{request}")))
        .transpose()
}

fn write_opening(tx: &Transaction, opening: &ReviewOpening) -> Result<(), StoreError> {
    let state = match opening.state {
        ReviewOpeningState::Preparing => "preparing",
        ReviewOpeningState::Published => "published",
        ReviewOpeningState::Interrupted => "interrupted",
        ReviewOpeningState::Failed => "failed",
        ReviewOpeningState::Cancelled => "cancelled",
    };
    tx.execute("INSERT INTO review_openings (project_key, request_id, task_id, number, version, state, record) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(project_key, request_id) DO UPDATE SET version = ?5, state = ?6, record = ?7", params![opening.project_path, opening.request_id, opening.task.id, opening.task.number as i64, opening.version as i64, state, serde_json::to_string(opening).expect("opening serializes")])?;
    Ok(())
}
