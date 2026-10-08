//! PR-specific merge preconditions at the target's Git ref transaction.
use super::{GitActionError, GitStepOutcome};
use crate::reviews::model::ReviewDirectory;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Merge the saved head only from the selected exact base. Callers retain
/// repository sequencing; finalization later reacquires receiver guards.
pub fn merge_expected(
    directory: &ReviewDirectory,
    source_path: &Path,
    target_ref: &str,
    expected_base: &str,
) -> Result<GitStepOutcome, GitActionError> {
    super::source_repo(directory, source_path).map_err(GitActionError::Failed)?;
    let branch = target_ref.strip_prefix("refs/heads/").ok_or_else(|| {
        GitActionError::Failed("PR target must be a full local branch ref".into())
    })?;
    super::valid_branch(source_path, branch).map_err(GitActionError::Failed)?;
    let expected = git2::Oid::from_str(expected_base)
        .map_err(|error| GitActionError::Failed(error.to_string()))?
        .to_string();
    let head = super::saved_head(directory).map_err(GitActionError::Failed)?;
    let imported =
        super::import_if_needed(directory, source_path, head).map_err(GitActionError::Failed)?;
    let result = expected_target(source_path, target_ref, &expected)
        .and_then(|()| merge_in_target_expected(source_path, branch, head, &expected));
    finish_typed(result, imported.cleanup(source_path))
}

/// Caller holds the native target ref reservation across this read and the
/// durable Merged transition. The source checkout may be on another branch.
pub fn verify_integrated(
    source_path: &Path,
    target_ref: &str,
    head: &str,
) -> Result<String, String> {
    if !target_ref.starts_with("refs/heads/") || !git2::Reference::is_valid_name(target_ref) {
        return Err("PR target must be a full local branch ref".into());
    }
    let repository = git2::Repository::open(source_path).map_err(|error| error.to_string())?;
    let reference = repository
        .find_reference(target_ref)
        .map_err(|error| error.to_string())?;
    let tip = reference.target().ok_or("PR target ref became symbolic")?;
    let received = git2::Oid::from_str(head).map_err(|error| error.to_string())?;
    repository
        .find_commit(tip)
        .map_err(|error| error.to_string())?;
    repository
        .find_commit(received)
        .map_err(|error| error.to_string())?;
    if tip != received
        && !repository
            .graph_descendant_of(tip, received)
            .map_err(|error| error.to_string())?
    {
        return Err(format!(
            "PR target {target_ref} does not contain its reviewed head {head}"
        ));
    }
    Ok(tip.to_string())
}

fn merge_in_target_expected(
    source: &Path,
    branch: &str,
    head: &str,
    expected: &str,
) -> Result<GitStepOutcome, GitActionError> {
    let repository = git2::Repository::open(source)
        .map_err(|error| GitActionError::Failed(error.to_string()))?;
    if let Some(reason) = super::unfinished_on_branch(&repository, branch) {
        return Err(GitActionError::Failed(reason));
    }
    let target_ref = format!("refs/heads/{branch}");
    if let Some(checkout) =
        super::checked_out_at(source, &target_ref).map_err(GitActionError::Failed)?
    {
        return merge_checkout_expected(&checkout, branch, head, expected);
    }
    merge_temporary_target(source, branch, head, expected)
}

fn merge_temporary_target(
    source: &Path,
    branch: &str,
    head: &str,
    expected: &str,
) -> Result<GitStepOutcome, GitActionError> {
    let owner = std::env::temp_dir().join(format!("build-review-merge-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&owner).map_err(|error| GitActionError::Failed(error.to_string()))?;
    let checkout = owner.join(format!("review-{}", uuid::Uuid::new_v4()));
    if let Err(error) = super::materialize_review_target(source, branch, &checkout) {
        let cleanup = fs::remove_dir(&owner);
        if checkout.exists() || cleanup.is_err() {
            return Err(GitActionError::Failed(format!(
                "{error}; temporary checkout may remain at {}",
                checkout.display()
            )));
        }
        if let Some(other) = super::checked_out_at(source, &format!("refs/heads/{branch}"))
            .map_err(GitActionError::Failed)?
        {
            return merge_checkout_expected(&other, branch, head, expected);
        }
        return Err(GitActionError::Failed(error));
    }
    let result = merge_checkout_expected(&checkout, branch, head, expected);
    let cleanup = super::remove_review_target(source, branch, &checkout).map_err(|error| {
        format!(
            "temporary checkout {} could not be removed: {error}",
            checkout.display()
        )
    });
    if cleanup.is_ok() {
        let _ = fs::remove_dir(&owner);
    }
    finish_typed(result, cleanup)
}

fn merge_checkout_expected(
    checkout: &Path,
    branch: &str,
    head: &str,
    expected: &str,
) -> Result<GitStepOutcome, GitActionError> {
    merge_checkout_expected_with_runner(checkout, branch, head, expected, super::merge_git_action)
}

fn merge_checkout_expected_with_runner(
    checkout: &Path,
    branch: &str,
    head: &str,
    expected_base: &str,
    run: impl FnOnce(&Path, &[&str]) -> Result<String, GitActionError>,
) -> Result<GitStepOutcome, GitActionError> {
    let target_ref = format!("refs/heads/{branch}");
    expected_target(checkout, &target_ref, expected_base)?;
    let fence = TargetFence::create(&target_ref, expected_base).map_err(GitActionError::Failed)?;
    let hooks = format!(
        "core.hooksPath={}",
        fence.path.to_str().ok_or_else(|| {
            GitActionError::Failed("PR ref transaction fence path is not UTF-8".into())
        })?
    );
    let mut failure = None;
    let result = super::merge_checkout_with_runner(checkout, branch, head, |path, args| {
        expected_target(path, &target_ref, expected_base)?;
        let mut arguments = vec!["-c", hooks.as_str()];
        arguments.extend_from_slice(args);
        let result = run(path, &arguments);
        failure = result.as_ref().err().cloned();
        result
    });
    result.map_err(|error| classify_failure(checkout, &target_ref, expected_base, failure, error))
}

fn expected_target(path: &Path, target_ref: &str, expected: &str) -> Result<(), GitActionError> {
    let repository =
        git2::Repository::open(path).map_err(|error| GitActionError::Failed(error.to_string()))?;
    let reference = repository
        .find_reference(target_ref)
        .map_err(|error| GitActionError::Failed(error.to_string()))?;
    if reference
        .target()
        .map(|target| target.to_string())
        .as_deref()
        != Some(expected)
    {
        return Err(GitActionError::Failed(format!(
            "PR target {target_ref} moved from its expected base {expected}"
        )));
    }
    Ok(())
}

fn classify_failure(
    checkout: &Path,
    target_ref: &str,
    expected: &str,
    failure: Option<GitActionError>,
    error: String,
) -> GitActionError {
    match failure {
        Some(GitActionError::OutcomeUnknown(_)) => GitActionError::OutcomeUnknown(error),
        Some(GitActionError::TimedOut(_) | GitActionError::TimedOutWithOwnedLock(_, _))
            if !restored_checkout(checkout, target_ref, expected) =>
        {
            GitActionError::OutcomeUnknown(error)
        }
        _ => GitActionError::Failed(error),
    }
}

fn restored_checkout(checkout: &Path, target_ref: &str, expected: &str) -> bool {
    let Ok(repository) = git2::Repository::open(checkout) else {
        return false;
    };
    repository.state() == git2::RepositoryState::Clean
        && repository.head().ok().is_some_and(|head| {
            head.name() == Some(target_ref)
                && head.target().map(|oid| oid.to_string()).as_deref() == Some(expected)
        })
        && super::git(
            checkout,
            &["status", "--porcelain", "--untracked-files=all"],
        )
        .is_ok_and(|status| status.is_empty())
}

fn finish_typed(
    result: Result<GitStepOutcome, GitActionError>,
    cleanup: Result<(), String>,
) -> Result<GitStepOutcome, GitActionError> {
    match (result, cleanup) {
        (Ok(outcome), cleanup) => {
            super::finish_with_cleanup(Ok(outcome), cleanup).map_err(GitActionError::Failed)
        }
        (Err(error), Ok(())) => Err(error),
        (Err(GitActionError::OutcomeUnknown(error)), Err(cleanup)) => Err(
            GitActionError::OutcomeUnknown(format!("{error}; cleanup failed: {cleanup}")),
        ),
        (Err(error), Err(cleanup)) => Err(GitActionError::Failed(format!(
            "{error}; cleanup failed: {cleanup}"
        ))),
    }
}

/// A per-command hook observes the old OID while Git holds its real target ref
/// lock. Repository hooks and configuration are never written by this fence.
struct TargetFence {
    path: PathBuf,
    directory: File,
    hook: Option<File>,
}

impl TargetFence {
    fn create(target_ref: &str, expected: &str) -> Result<Self, String> {
        if !git2::Reference::is_valid_name(target_ref)
            || !target_ref.starts_with("refs/heads/")
            || git2::Oid::from_str(expected).is_err()
        {
            return Err("invalid PR target ref transaction fence".into());
        }
        let path =
            std::env::temp_dir().join(format!("build-review-fence-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path).map_err(|error| error.to_string())?;
        let path = path.canonicalize().map_err(|error| error.to_string())?;
        let mut guard = Self {
            directory: File::open(&path).map_err(|error| error.to_string())?,
            path,
            hook: None,
        };
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o700);
        }
        let mut file = options
            .open(guard.path.join("reference-transaction"))
            .map_err(|error| error.to_string())?;
        let script = fence_script(target_ref, expected);
        file.write_all(script.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| error.to_string())?;
        drop(file);
        guard.hook = Some(
            File::open(guard.path.join("reference-transaction"))
                .map_err(|error| error.to_string())?,
        );
        guard
            .directory
            .sync_all()
            .map_err(|error| error.to_string())?;
        Ok(guard)
    }
}

fn fence_script(target_ref: &str, expected: &str) -> String {
    let target = format!("'{}'", target_ref.replace('\'', "'\\''"));
    format!(
        "#!/bin/sh\n[ \"$1\" = prepared ] || exit 0\n\
         while IFS=' ' read -r old new reference; do\n\
         case \"$reference\" in\n\
         {target}|HEAD)\n\
         if [ \"$old\" != '{expected}' ]; then\n\
         printf '%s\\n' 'PR merge target moved after its final check' >&2\n\
         exit 1\nfi\n;;\n\
         refs/heads/*)\n\
         printf '%s\\n' 'PR merge target changed its branch binding' >&2\n\
         exit 1\n;;\nesac\ndone\n"
    )
}

impl Drop for TargetFence {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let same = |opened: &File, path: &Path| {
                opened
                    .metadata()
                    .ok()
                    .zip(fs::symlink_metadata(path).ok())
                    .is_some_and(|(opened, current)| {
                        !current.file_type().is_symlink()
                            && opened.dev() == current.dev()
                            && opened.ino() == current.ino()
                    })
            };
            if !same(&self.directory, &self.path)
                || self
                    .hook
                    .as_ref()
                    .is_some_and(|hook| !same(hook, &self.path.join("reference-transaction")))
            {
                return;
            }
        }
        let _ = fs::remove_file(self.path.join("reference-transaction"));
        let _ = fs::remove_dir(&self.path);
    }
}

#[cfg(test)]
mod tests;
