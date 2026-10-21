//! Session-owned uploads. App owns admission; file services own staging I/O.
use super::deferred::{FileTarget, PreparedFile};
use super::{AppState, FileScope};
use crate::api::v1::git::{FsUploadBeginParams, FsUploadChunkParams, FsUploadIdParams};
use crate::api::ApiError;
use crate::carrier::{Opening, SessionSender};
use crate::scoped_upload::{Destination, StagedFile};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(in crate::app) const UPLOAD_MAX_BYTES: u64 = 256 * 1_048_576;
pub(in crate::app) const UPLOAD_CHUNK_BYTES: u64 = 4 * 1_048_576;
const UPLOAD_SESSION_LIMIT: usize = 8;
const UPLOAD_TOTAL_LIMIT: usize = 64;
const ACTIVE: u8 = 0;
const CANCELLED: u8 = 1;
const PUBLISHING: u8 = 2;
const UPLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

type UploadFailure = (ApiError, bool);
type ReopenResult = Result<Option<crate::workspace::FileReopen>, ApiError>;
type FinishedUpload = (String, ReopenResult);

struct UploadFile {
    staged: StagedFile,
    received: u64,
}

/// Clones reserve quota and keep staging alive while an off-lock service runs.
/// No app-lock path takes `file`; source serialization precedes its acquisition.
#[derive(Clone)]
pub(in crate::app) struct Upload {
    target: FileTarget,
    root: PathBuf,
    path: String,
    owner: String,
    opening: Opening,
    size: u64,
    last_chunk_at: Instant,
    file: Arc<Mutex<Option<UploadFile>>>,
    lifecycle: Arc<AtomicU8>,
}

impl Upload {
    fn validate(&self) -> Result<(), ApiError> {
        if !self.active() || !self.opening.is_open() {
            return Err(ApiError::not_found("unknown upload_id"));
        }
        Ok(())
    }

    fn validate_scope(&self, app: &AppState, root: &Path) -> Result<(), ApiError> {
        let checked = self.target.validate(app, root);
        if let Err(error) = checked {
            let explicit_path = matches!(
                &self.target.scope,
                Some(FileScope::ProjectSource {
                    source_path: Some(_),
                    ..
                })
            );
            if error.code() == "conflict" && !explicit_path {
                return Err(ApiError::invalid_params(
                    "upload scope changed since upload began",
                ));
            }
            return Err(error);
        }
        if root != self.root {
            return Err(ApiError::invalid_params(
                "upload scope changed since upload began",
            ));
        }
        Ok(())
    }

    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.file, &other.file)
    }

    fn active(&self) -> bool {
        self.lifecycle.load(Ordering::Acquire) == ACTIVE
    }

    fn try_cancel(&self) -> Result<(), ApiError> {
        self.lifecycle
            .compare_exchange(ACTIVE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| ApiError::not_found("unknown upload_id"))
    }

    fn cancel(&self) {
        let _ = self.try_cancel();
    }

    /// Claim the attempt before staging fsync/rename. Cancellation wins before
    /// this point; afterwards finish owns disk and cache settlement even if
    /// its session closes. A claimed attempt can still return a disk error.
    fn claim_publication(&self) -> Result<(), ApiError> {
        self.validate()?;
        self.lifecycle
            .compare_exchange(ACTIVE, PUBLISHING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| ApiError::not_found("unknown upload_id"))?;
        if !self.opening.is_open() {
            self.lifecycle.store(CANCELLED, Ordering::Release);
            return Err(ApiError::not_found("unknown upload_id"));
        }
        Ok(())
    }

    pub(in crate::app) fn dispose(self) {
        self.cancel();
        self.file.lock().unwrap().take();
    }
}

impl AppState {
    pub fn sweep_stale_uploads(&self) -> usize {
        let mut roots: HashSet<PathBuf> = self.worktree_roots().into_values().collect();
        for project in self.projects.iter() {
            roots.insert(project.repo_path.clone());
            roots.extend(project.sources.iter().map(|source| source.path.clone()));
        }
        for workspace in self.workspaces.list(None) {
            roots.insert(workspace.root.clone());
            roots.extend(
                workspace
                    .directories
                    .iter()
                    .map(|directory| directory.path.clone()),
            );
        }
        for (id, run) in &self.runs {
            roots.insert(self.run_git_root(id, &run.worktree.path));
        }
        let roots: HashSet<PathBuf> = roots
            .into_iter()
            .filter_map(|root| std::fs::canonicalize(root).ok())
            .collect();
        let active: HashSet<String> = self.uploads.keys().cloned().collect();
        roots
            .iter()
            .map(|root| crate::scoped_upload::remove_stale_uploads(root, &active))
            .sum()
    }

    pub(in crate::app) fn drop_ended_session_uploads(&mut self, session_id: &str) -> Vec<Upload> {
        self.take_uploads(|upload| upload.owner == session_id && !upload.opening.is_open())
    }

    fn take_uploads(&mut self, remove: impl Fn(&Upload) -> bool) -> Vec<Upload> {
        let ids: Vec<String> = self
            .uploads
            .iter()
            .filter(|(_, upload)| remove(upload))
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter()
            .filter_map(|id| self.uploads.remove(&id))
            .inspect(Upload::cancel)
            .collect()
    }

    pub(in crate::app) fn upload_open_within(&self, root: &Path) -> bool {
        self.uploads
            .values()
            .any(|upload| upload.root.starts_with(root))
    }

    pub fn spawn_upload_reaper(state: Arc<Mutex<AppState>>, interval: Duration) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let sweeping = Arc::clone(&state);
                crate::app::runtime::off_the_workers::off_the_workers(move || {
                    let removed = sweeping.lock().unwrap().take_idle_uploads(Instant::now());
                    for upload in removed {
                        upload.dispose();
                    }
                })
                .await;
            }
        });
    }

    fn take_idle_uploads(&mut self, now: Instant) -> Vec<Upload> {
        self.take_uploads(|upload| {
            !upload.opening.is_open()
                || now.saturating_duration_since(upload.last_chunk_at) >= UPLOAD_IDLE_TIMEOUT
        })
    }

    #[cfg(test)]
    pub(in crate::app) fn reap_idle_uploads_at(&mut self, now: Instant) -> usize {
        let removed = self.take_idle_uploads(now);
        let count = removed.len();
        for upload in removed {
            upload.dispose();
        }
        count
    }

    fn owned_upload(&self, id: &str, caller: &SessionSender) -> Result<&Upload, ApiError> {
        self.uploads
            .get(id)
            .filter(|upload| {
                upload.active()
                    && upload.opening.is_open()
                    && upload.owner == caller.session_id()
                    && upload.opening.is(&caller.opening())
            })
            .ok_or_else(|| ApiError::not_found("unknown upload_id"))
    }

    fn upload_is_current(&self, id: &str, upload: &Upload) -> Result<(), ApiError> {
        upload.validate()?;
        self.uploads
            .get(id)
            .filter(|current| current.same(upload))
            .map(|_| ())
            .ok_or_else(|| ApiError::not_found("unknown upload_id"))
    }

    pub(crate) fn fs_upload_abort(
        &mut self,
        params: FsUploadIdParams,
        caller: &SessionSender,
    ) -> Result<Value, ApiError> {
        // The CAS decides cancellation while the map entry is still present.
        // A finish may have claimed publication after owned_upload's first read.
        self.owned_upload(&params.upload_id, caller)?.try_cancel()?;
        let upload = self
            .uploads
            .remove(&params.upload_id)
            .expect("owned upload");
        Ok(self.defer_file(
            move || {
                #[cfg(test)]
                super::service_test::service();
                // A preceding chunk/finish owns another reference; cancellation
                // prevents it publishing and its off-lock drop owns cleanup.
                upload.dispose();
                json!({})
            },
            |_app, answer| Ok(answer),
        ))
    }

    pub(crate) fn fs_upload_begin(
        &mut self,
        params: FsUploadBeginParams,
        caller: &SessionSender,
    ) -> Result<Value, ApiError> {
        let target = FileTarget::capture(
            self,
            FileScope::parse(&crate::api::v1::WireParams::wire(&params.scope))
                .map_err(ApiError::classify)?,
        )?;
        let removed = self.take_idle_uploads(Instant::now());
        if !caller.session_is_open() {
            // No staging can be dropped under app on an admission refusal.
            return Ok(self.defer_file(
                move || {
                    for upload in removed {
                        upload.dispose();
                    }
                },
                |_app, ()| Err(ApiError::not_found("client session has ended")),
            ));
        }
        let owned = self
            .uploads
            .values()
            .filter(|upload| upload.owner == caller.session_id())
            .count();
        if self.uploads.len() >= UPLOAD_TOTAL_LIMIT || owned >= UPLOAD_SESSION_LIMIT {
            return Ok(self.defer_file(
                move || {
                    for upload in removed {
                        upload.dispose();
                    }
                },
                |_app, ()| {
                    Err(ApiError::busy(
                        "too many open uploads; finish or abort an upload first",
                    ))
                },
            ));
        }
        if params.size > UPLOAD_MAX_BYTES {
            return Ok(self.defer_file(
                move || {
                    for upload in removed {
                        upload.dispose();
                    }
                },
                |_app, ()| {
                    Err(ApiError::invalid_params(
                        "upload size exceeds the 268435456-byte limit",
                    ))
                },
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let upload = Upload {
            root: target.recorded_root.clone(),
            path: if params.parent.is_empty() {
                params.name.clone()
            } else {
                format!("{}/{}", params.parent, params.name)
            },
            target: target.clone(),
            owner: caller.session_id().into(),
            opening: caller.opening(),
            size: params.size,
            last_chunk_at: Instant::now(),
            file: Arc::new(Mutex::new(None)),
            lifecycle: Arc::new(AtomicU8::new(ACTIVE)),
        };
        self.uploads.insert(id.clone(), upload.clone());
        // Clean expired staging on the same off-lock preparation trip.
        let captured = target.clone();
        Ok(self.defer_file(
            move || {
                for upload in removed {
                    upload.dispose();
                }
                captured.prepare()
            },
            move |app, prepared| {
                let result = app.start_upload_begin(id.clone(), upload, params, prepared);
                if result.is_err() {
                    app.uploads.remove(&id);
                }
                result
            },
        ))
    }

    fn discard_upload(&mut self, id: String, upload: Upload, error: ApiError) -> Value {
        if self
            .uploads
            .get(&id)
            .is_some_and(|current| current.same(&upload))
        {
            self.uploads.remove(&id);
        }
        upload.cancel();
        self.defer_file(move || upload.dispose(), |_app, ()| Err(error))
    }

    fn start_upload_begin(
        &mut self,
        id: String,
        mut upload: Upload,
        params: FsUploadBeginParams,
        prepared: Result<PreparedFile, ApiError>,
    ) -> Result<Value, ApiError> {
        let prepared = prepared?;
        self.upload_is_current(&id, &upload)?;
        upload.target.validate(self, &prepared.root)?;
        upload.root = prepared.root.clone();
        self.uploads.get_mut(&id).expect("admitted upload").root = prepared.root.clone();
        let work = upload.clone();
        Ok(self.defer_file(
            move || {
                #[cfg(test)]
                super::service_test::service();
                work.validate()?;
                let destination = Destination::open(&prepared.root, &params.parent, &params.name)?;
                let path = destination.path().to_string();
                let mut file = work.file.lock().unwrap();
                work.validate()?;
                let staged = destination.stage(&id, params.replace.unwrap_or(false))?;
                *file = Some(UploadFile {
                    staged,
                    received: 0,
                });
                drop(file);
                Ok((id, path, work, prepared))
            },
            move |app, result: Result<_, ApiError>| match result {
                Ok((id, path, work, _prepared)) => {
                    let checked = app
                        .upload_is_current(&id, &work)
                        .and_then(|()| work.target.validate(app, &_prepared.root));
                    if let Err(error) = checked {
                        return Ok(app.discard_upload(id, work, error));
                    }
                    Ok(json!({"upload_id":id,"path":path,"chunk_bytes":UPLOAD_CHUNK_BYTES}))
                }
                Err(error) => {
                    app.uploads.retain(|_, current| !current.same(&upload));
                    Err(error)
                }
            },
        ))
    }

    pub(crate) fn fs_upload_chunk(
        &mut self,
        params: FsUploadChunkParams,
        caller: &SessionSender,
    ) -> Result<Value, ApiError> {
        let upload = self.owned_upload(&params.upload_id, caller)?.clone();
        let target = upload.target.clone();
        Ok(self.prepare_file(target, move |app, _target, prepared| {
            let prepared = prepared?;
            app.upload_is_current(&params.upload_id, &upload)?;
            upload.validate_scope(app, &prepared.root)?;
            let work = upload.clone();
            Ok(app.defer_file(
                move || {
                    let result = write_upload_chunk(&work, &params, &prepared.root);
                    if result.as_ref().is_err_and(|(_, fatal)| *fatal) {
                        work.file.lock().unwrap().take();
                    }
                    (result, params.upload_id, work, prepared)
                },
                move |app, (result, id, work, _prepared)| {
                    if let Err(error) = app.upload_is_current(&id, &work) {
                        return Ok(app.discard_upload(id, work, error));
                    }
                    match result {
                        Ok(received) => {
                            app.uploads
                                .get_mut(&id)
                                .expect("owned upload")
                                .last_chunk_at = Instant::now();
                            Ok(json!({"received":received}))
                        }
                        Err((error, fatal)) => {
                            if fatal {
                                app.uploads.remove(&id);
                            }
                            Err(error)
                        }
                    }
                },
            ))
        }))
    }

    pub(crate) fn fs_upload_finish(
        &mut self,
        params: FsUploadIdParams,
        caller: &SessionSender,
    ) -> Result<Value, ApiError> {
        let upload = self.owned_upload(&params.upload_id, caller)?.clone();
        let mut target = upload.target.clone();
        target.destination = Some(upload.path.clone());
        Ok(self.prepare_file(target, move |app, target, prepared| {
            let prepared = match prepared.and_then(|prepared| {
                app.upload_is_current(&params.upload_id, &upload)?;
                upload.validate_scope(app, &prepared.root)?;
                Ok(prepared)
            }) {
                Ok(prepared) => prepared,
                Err(error) if error.code() == "busy" => return Err(error),
                Err(error) => return Ok(app.discard_upload(params.upload_id, upload, error)),
            };
            let reopened = app.file_reopen(&target)?;
            let work = upload.clone();
            let root = prepared.root.clone();
            Ok(app.defer_file(
                move || {
                    let result = finish_upload(&work, &root, reopened);
                    if result.as_ref().is_err_and(|(_, fatal)| *fatal) {
                        work.file.lock().unwrap().take();
                    }
                    (result, params.upload_id, work, prepared)
                },
                move |app, (result, id, work, prepared)| {
                    app.settle_upload_finish(result, id, work, &target, &prepared.root)
                },
            ))
        }))
    }

    fn settle_upload_finish(
        &mut self,
        result: Result<FinishedUpload, UploadFailure>,
        id: String,
        upload: Upload,
        target: &FileTarget,
        root: &Path,
    ) -> Result<Value, ApiError> {
        match result {
            Ok((path, reopened)) => {
                // The publication claim already ordered finish before cancellation.
                // Removing an absent entry cannot resurrect an ended opening.
                self.uploads.remove(&id);
                self.publish_file_mutation(target, root, reopened)?;
                Ok(json!({"path":path,"size":upload.size}))
            }
            Err((error, true)) => {
                self.uploads.remove(&id);
                Err(error)
            }
            Err((error, false)) => {
                if let Err(ended) = self.upload_is_current(&id, &upload) {
                    return Ok(self.discard_upload(id, upload, ended));
                }
                Err(error)
            }
        }
    }
}

fn write_upload_chunk(
    upload: &Upload,
    params: &FsUploadChunkParams,
    root: &Path,
) -> Result<u64, (ApiError, bool)> {
    #[cfg(test)]
    super::service_test::service();
    let checked = || -> Result<Vec<u8>, ApiError> {
        upload.validate()?;
        if root != upload.root {
            return Err(ApiError::invalid_params(
                "upload scope changed since upload began",
            ));
        }
        if params.content_b64.len() as u64 > UPLOAD_CHUNK_BYTES.div_ceil(3) * 4 {
            return Err(ApiError::invalid_params(
                "upload chunk exceeds the 4194304-byte limit",
            ));
        }
        crate::encoding::b64decode(&params.content_b64).map_err(ApiError::invalid_params)
    };
    let bytes = checked().map_err(|error| (error, false))?;
    let mut file = upload.file.lock().unwrap();
    let file = file
        .as_mut()
        .ok_or_else(|| (ApiError::busy("upload is still beginning"), false))?;
    file.staged
        .validate_parent(root)
        .map_err(|error| (error, false))?;
    if params.offset != file.received {
        return Err((
            ApiError::invalid_params("upload offset must equal received bytes"),
            false,
        ));
    }
    if bytes.len() as u64 > UPLOAD_CHUNK_BYTES || file.received + bytes.len() as u64 > upload.size {
        return Err((
            ApiError::invalid_params("upload chunk exceeds chunk size or declared upload size"),
            false,
        ));
    }
    upload.validate().map_err(|error| (error, false))?;
    file.staged
        .write_chunk(params.offset, &bytes)
        .map_err(|error| (error, true))?;
    file.received += bytes.len() as u64;
    Ok(file.received)
}

fn finish_upload(
    upload: &Upload,
    root: &Path,
    reopened: Option<crate::workspace::FileReopen>,
) -> Result<FinishedUpload, UploadFailure> {
    #[cfg(test)]
    super::service_test::service();
    upload.validate().map_err(|error| (error, true))?;
    if root != upload.root {
        return Err((
            ApiError::invalid_params("upload scope changed since upload began"),
            true,
        ));
    }
    let mut file = upload.file.lock().unwrap();
    let active = file
        .as_mut()
        .ok_or_else(|| (ApiError::busy("upload is still beginning"), false))?;
    if active.received != upload.size {
        return Err((
            ApiError::invalid_params("upload must receive the declared size before finishing"),
            false,
        ));
    }
    active
        .staged
        .validate_parent(root)
        .map_err(|error| (error, true))?;
    #[cfg(test)]
    super::service_test::before_publish();
    upload.claim_publication().map_err(|error| (error, true))?;
    #[cfg(test)]
    super::service_test::after_publish_claim();
    let path = active
        .staged
        .finish(upload.size)
        .map_err(|error| (error, true))?;
    // Remove the published staging descriptor off-lock, including failures.
    file.take();
    Ok((path, super::deferred::persist_reopen(reopened)))
}
