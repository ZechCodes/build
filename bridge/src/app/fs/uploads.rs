//! Session-owned uploads; staging files clean themselves up when their state is dropped.

use super::{AppState, FileScope};
use crate::api::v1::git::{
    FsUploadAbortResult, FsUploadBeginParams, FsUploadBeginResult, FsUploadChunkParams,
    FsUploadChunkResult, FsUploadFinishResult, FsUploadIdParams,
};
use crate::api::ApiError;
use crate::carrier::{Opening, SessionSender};
use crate::scoped_upload::{Destination, StagedFile};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(in crate::app) const UPLOAD_MAX_BYTES: u64 = 256 * 1_048_576;
pub(in crate::app) const UPLOAD_CHUNK_BYTES: u64 = 4 * 1_048_576;
const UPLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub(in crate::app) struct Upload {
    scope: FileScope,
    root: PathBuf,
    owner: String,
    opening: Opening,
    size: u64,
    received: u64,
    last_chunk_at: Instant,
    staged: StagedFile,
}

impl AppState {
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
                    sweeping
                        .lock()
                        .unwrap()
                        .reap_idle_uploads_at(Instant::now())
                })
                .await;
            }
        });
    }

    pub(in crate::app) fn reap_idle_uploads_at(&mut self, now: Instant) -> usize {
        let before = self.uploads.len();
        self.uploads.retain(|_, upload| {
            now.saturating_duration_since(upload.last_chunk_at) < UPLOAD_IDLE_TIMEOUT
        });
        before - self.uploads.len()
    }

    pub(crate) fn fs_upload_abort(
        &mut self,
        params: FsUploadIdParams,
        caller: &SessionSender,
    ) -> Result<FsUploadAbortResult, ApiError> {
        let upload = self.owned_upload(&params.upload_id, caller)?;
        self.refuse_writers_while_reserved(&upload.root)
            .map_err(ApiError::classify)?;
        self.uploads.remove(&params.upload_id);
        Ok(FsUploadAbortResult {})
    }

    pub(crate) fn fs_upload_finish(
        &mut self,
        params: FsUploadIdParams,
        caller: &SessionSender,
    ) -> Result<FsUploadFinishResult, ApiError> {
        self.owned_upload(&params.upload_id, caller)?;
        if let Err(error) = self.validate_upload_scope(&params.upload_id, caller) {
            if error.code() != "busy" {
                self.uploads.remove(&params.upload_id);
            }
            return Err(error);
        }
        let upload = self.owned_upload(&params.upload_id, caller)?;
        if upload.received != upload.size {
            return Err(ApiError::invalid_params(
                "upload must receive the declared size before finishing",
            ));
        }
        let mut upload = self
            .uploads
            .remove(&params.upload_id)
            .expect("validated upload");
        let path = upload.staged.finish(upload.size)?;
        self.finish_file_mutation(&upload.scope)?;
        Ok(FsUploadFinishResult {
            path,
            size: upload.size,
        })
    }

    fn owned_upload(&self, id: &str, caller: &SessionSender) -> Result<&Upload, ApiError> {
        self.uploads
            .get(id)
            .filter(|upload| {
                upload.owner == caller.session_id() && upload.opening.is(&caller.opening())
            })
            .ok_or_else(|| ApiError::not_found("unknown upload_id"))
    }

    fn validate_upload_scope(&mut self, id: &str, caller: &SessionSender) -> Result<(), ApiError> {
        let upload = self.owned_upload(id, caller)?;
        let (scope, root) = (upload.scope.clone(), upload.root.clone());
        self.refuse_writers_while_reserved(&root)
            .map_err(ApiError::classify)?;
        let current = scope.resolve_root(self).map_err(ApiError::classify)?;
        let current = std::fs::canonicalize(current).map_err(|error| {
            ApiError::invalid_params(format!("cannot resolve upload root: {error}"))
        })?;
        if current != root {
            return Err(ApiError::invalid_params(
                "upload scope changed since upload began",
            ));
        }
        self.owned_upload(id, caller)?
            .staged
            .validate_parent(&current)
    }

    pub(crate) fn fs_upload_chunk(
        &mut self,
        params: FsUploadChunkParams,
        caller: &SessionSender,
    ) -> Result<FsUploadChunkResult, ApiError> {
        self.validate_upload_scope(&params.upload_id, caller)?;
        let upload = self
            .uploads
            .get_mut(&params.upload_id)
            .expect("validated upload");
        if params.offset != upload.received {
            return Err(ApiError::invalid_params(
                "upload offset must equal received bytes",
            ));
        }
        if params.content_b64.len() as u64 > UPLOAD_CHUNK_BYTES.div_ceil(3) * 4 {
            return Err(ApiError::invalid_params(
                "upload chunk exceeds the 4194304-byte limit",
            ));
        }
        let bytes =
            crate::encoding::b64decode(&params.content_b64).map_err(ApiError::invalid_params)?;
        if bytes.len() as u64 > UPLOAD_CHUNK_BYTES
            || upload.received + bytes.len() as u64 > upload.size
        {
            return Err(ApiError::invalid_params(
                "upload chunk exceeds chunk size or declared upload size",
            ));
        }
        if let Err(error) = upload.staged.write_chunk(params.offset, &bytes) {
            self.uploads.remove(&params.upload_id);
            return Err(error);
        }
        upload.received += bytes.len() as u64;
        upload.last_chunk_at = Instant::now();
        Ok(FsUploadChunkResult {
            received: upload.received,
        })
    }

    pub(crate) fn fs_upload_begin(
        &mut self,
        params: FsUploadBeginParams,
        caller: &SessionSender,
    ) -> Result<FsUploadBeginResult, ApiError> {
        if params.size > UPLOAD_MAX_BYTES {
            return Err(ApiError::invalid_params(
                "upload size exceeds the 268435456-byte limit",
            ));
        }
        let (scope, root) = self.file_mutation_scope(&params.scope)?;
        let destination = Destination::open(&root, &params.parent, &params.name)?;
        let path = destination.path().to_string();
        let root = std::fs::canonicalize(root)
            .map_err(|error| ApiError::internal(format!("cannot resolve upload root: {error}")))?;
        let upload_id = uuid::Uuid::new_v4().to_string();
        let staged = destination.stage(&upload_id, params.replace.unwrap_or(false))?;
        self.uploads.insert(
            upload_id.clone(),
            Upload {
                scope,
                root,
                owner: caller.session_id().into(),
                opening: caller.opening(),
                size: params.size,
                received: 0,
                last_chunk_at: Instant::now(),
                staged,
            },
        );
        Ok(FsUploadBeginResult {
            upload_id,
            path,
            chunk_bytes: UPLOAD_CHUNK_BYTES,
        })
    }
}
