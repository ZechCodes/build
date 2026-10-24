use super::*;
use crate::reviews::receivers::{
    locks::{acquire_git_lock_with_wait, OwnedGitFileLock, RecoveryWait},
    storage,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};

pub(super) struct Worker {
    store: Store,
    root: PathBuf,
    after: Option<PathBuf>,
    after_deletion: Option<PathBuf>,
    collected: BTreeMap<PathBuf, std::time::Instant>,
    #[cfg(test)]
    pub(super) now: u64,
}

enum Work {
    Collect(PathBuf),
    Delete(deletion::Job),
    None,
}

enum Retirement {
    Retained,
    Released(Box<leases::ReviewLeases>),
}

impl Worker {
    pub(super) fn new(store: Store, root: PathBuf) -> Self {
        Self {
            store,
            root,
            after: None,
            after_deletion: None,
            collected: BTreeMap::new(),
            #[cfg(test)]
            now: 0,
        }
    }

    pub(super) fn turn(&mut self) -> Result<Report, String> {
        if !self.root.exists() {
            return Ok(Report::default());
        }
        let Some(opening) = self.opening_lease()? else {
            return Ok(Report::default());
        };
        let resumed = deletion::resume(&self.root, self.after_deletion.as_deref())?;
        if let Some(job) = &resumed {
            self.after_deletion = Some(job.journal_path().into());
        }
        let work = match self.next_receiver()? {
            Some(receiver) => self.prepare(&receiver),
            None => Ok(Work::None),
        };
        // Receiver admission is never excluded during the expensive deletion,
        // allocation walk or GC. Canonical receiver paths may be recreated.
        drop(opening);
        let mut report = Report::default();
        if let Some(job) = resumed {
            if let Err(error) = Self::finish_delete(job, &mut report) {
                eprintln!("review receivers: retained deletion job for retry: {error}");
            }
        }
        match work? {
            Work::Delete(job) => Self::finish_delete(job, &mut report)?,
            Work::Collect(receiver) => self.collect(&receiver, &mut report)?,
            Work::None => {}
        }
        Ok(report)
    }

    fn opening_lease(&self) -> Result<Option<leases::Lease>, String> {
        if self
            .root
            .canonicalize()
            .map_err(|error| error.to_string())?
            != self.root
        {
            return Err("receiver root placement changed".into());
        }
        let operations = self.root.join("operations");
        fs::create_dir_all(&operations).map_err(|error| error.to_string())?;
        if !fs::symlink_metadata(&operations)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_dir()
        {
            return Err("receiver operations placement changed".into());
        }
        let path = operations.join("opening.lock");
        drop(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|error| error.to_string())?,
        );
        match leases::Lease::acquire(&path) {
            Ok(lease) => Ok(Some(lease)),
            Err(error) if error.starts_with("busy:") => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn next_receiver(&mut self) -> Result<Option<PathBuf>, String> {
        let mut receivers = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.extension().is_some_and(|extension| extension == "git") {
                receivers.push(path);
            }
        }
        receivers.sort();
        let next = receivers
            .iter()
            .find(|path| self.after.as_ref().is_none_or(|after| *path > after))
            .or_else(|| receivers.first())
            .cloned();
        self.after = next.clone();
        Ok(next)
    }

    fn prepare(&mut self, receiver: &Path) -> Result<Work, String> {
        crate::reviews::receivers::validate_recorded_receiver_with_recovery_wait(
            receiver,
            RecoveryWait::TryOnly,
        )?;
        admission::check_closed_state(receiver)?;
        let reader = self
            .store
            .receiver_maintenance_reader()
            .map_err(|error| error.to_string())?;
        if reader
            .has_pending_opening(receiver)
            .map_err(|error| error.to_string())?
        {
            return Ok(Work::None);
        }
        let reviews = reader
            .reviews_at(receiver)
            .map_err(|error| error.to_string())?;
        let mut retained = false;
        let mut held = Vec::new();
        for review in &reviews {
            match self.retire(receiver, review)? {
                Retirement::Retained => retained = true,
                Retirement::Released(lease) => held.push(lease),
            }
        }
        if retained || has_foreign_refs(receiver)? {
            return Ok(Work::Collect(receiver.into()));
        }
        if !admission::ready(receiver, self.now_seconds()?)? {
            return Ok(Work::None);
        }
        // Native writers cannot update any existing or registered received ref
        // between the final metadata check and the canonical directory rename.
        let _refs = fence_receiver(receiver, &reviews)?;
        if has_foreign_refs(receiver)? {
            return Ok(Work::Collect(receiver.into()));
        }
        let job = deletion::prepare(&self.root, receiver)?;
        self.collected.remove(receiver);
        drop(held);
        Ok(Work::Delete(job))
    }

    fn retire(&mut self, receiver: &Path, observed: &Review) -> Result<Retirement, String> {
        if retention::retained(observed)? {
            return Ok(Retirement::Retained);
        }
        let reader = self
            .store
            .receiver_maintenance_reader()
            .map_err(|error| error.to_string())?;
        let pending = reader
            .has_candidate(&observed.task_id)
            .map_err(|error| error.to_string())?;
        let held = match leases::review(pending, observed) {
            Ok(held) => held,
            // A live or recovering capture/merge is retained for a later turn.
            Err(_) => return Ok(Retirement::Retained),
        };
        if reader
            .has_candidate(&observed.task_id)
            .map_err(|error| error.to_string())?
        {
            return Ok(Retirement::Retained);
        }
        let current = reader
            .reviews_at(receiver)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|review| review.task_id == observed.task_id)
            .ok_or("review disappeared during reclamation")?;
        if retention::retained(&current)?
            || reader
                .merge_unsettled(&current)
                .map_err(|error| error.to_string())?
        {
            return Ok(Retirement::Retained);
        }
        if refs::cleanup(receiver, &current, &self.root)? {
            self.collected.remove(receiver);
        }
        Ok(Retirement::Released(Box::new(held)))
    }

    fn collect(&mut self, receiver: &Path, report: &mut Report) -> Result<(), String> {
        if self
            .collected
            .get(receiver)
            .is_some_and(|collected| collected.elapsed() < std::time::Duration::from_secs(3600))
        {
            return Ok(());
        }
        storage::release_unshared_pack_keeps(receiver)?;
        let before = size::allocated(receiver)?;
        // prune=now can delete a native receive-pack's migrated objects between
        // connectivity checking and ref publication. A grace period protects
        // concurrent writers; terminal receivers are removed in full instead.
        collect_git(receiver)?;
        report.freed_bytes += before.saturating_sub(size::allocated(receiver)?);
        report.collected += 1;
        self.collected
            .insert(receiver.into(), std::time::Instant::now());
        Ok(())
    }

    fn finish_delete(job: deletion::Job, report: &mut Report) -> Result<(), String> {
        report.freed_bytes = report.freed_bytes.saturating_add(job.finish()?);
        report.removed += 1;
        Ok(())
    }

    fn now_seconds(&self) -> Result<u64, String> {
        #[cfg(test)]
        return Ok(self.now);
        #[cfg(not(test))]
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .map_err(|error| error.to_string())
    }

    #[cfg(test)]
    pub(super) fn turn_after_grace(&mut self) -> Result<Report, String> {
        let mut report = self.retry_turn()?;
        self.now += admission::ADMISSION_DRAIN_SECS;
        let next = self.retry_turn()?;
        report.freed_bytes += next.freed_bytes;
        report.removed += next.removed;
        report.collected += next.collected;
        Ok(report)
    }

    #[cfg(test)]
    pub(super) fn retry_turn(&mut self) -> Result<Report, String> {
        // Model later service turns after transient native recovery contention.
        // Direct turn() tests still verify immediate nonblocking admission.
        for _ in 0..100 {
            match self.turn() {
                Err(error) if error.starts_with("busy:") => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                result => return result,
            }
        }
        self.turn()
    }
}

fn collect_git(receiver: &Path) -> Result<(), String> {
    use std::ffi::OsStr;
    let arguments = ["gc", "--prune=2.weeks.ago"].map(OsStr::new);
    let output = crate::git_process::run_git_unattended(
        receiver,
        &arguments,
        std::time::Duration::from_secs(300),
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(crate::git_process::git_failure(&arguments, &output).to_string());
    }
    Ok(())
}

pub(super) fn has_foreign_refs(receiver: &Path) -> Result<bool, String> {
    let repository = git2::Repository::open_bare(receiver).map_err(|error| error.to_string())?;
    for reference in repository.references().map_err(|error| error.to_string())? {
        let reference = reference.map_err(|error| error.to_string())?;
        if reference.name().is_none_or(|name| {
            !name.starts_with("refs/build/reviews/") && !name.starts_with("refs/heads/review/")
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn fence_receiver(receiver: &Path, reviews: &[Review]) -> Result<Vec<OwnedGitFileLock>, String> {
    let repository = git2::Repository::open_bare(receiver).map_err(|error| error.to_string())?;
    let mut names = BTreeSet::new();
    for reference in repository.references().map_err(|error| error.to_string())? {
        let reference = reference.map_err(|error| error.to_string())?;
        names.insert(
            reference
                .name()
                .ok_or("receiver ref is not UTF-8")?
                .to_owned(),
        );
    }
    for binding in reviews
        .iter()
        .flat_map(|review| &review.bindings)
        .filter(|binding| binding.receiving_repository == receiver)
    {
        names.insert(binding.receiving_ref.clone());
    }
    names.insert("packed-refs".into());
    names
        .iter()
        .map(|name| {
            acquire_git_lock_with_wait(
                receiver,
                Path::new(&format!("{name}.lock")),
                RecoveryWait::TryOnly,
            )
            .map(|(guard, _)| guard)
        })
        .collect()
}
