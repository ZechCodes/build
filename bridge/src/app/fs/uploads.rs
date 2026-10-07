//! Session-owned uploads; staging files clean themselves up when their state is dropped.

use super::{AppState, FileScope};
use crate::api::v1::git::{FsUploadBeginParams, FsUploadBeginResult};
use crate::api::ApiError;
use crate::carrier::{Opening, SessionSender};
use crate::scoped_upload::{Destination, StagedFile};
use std::path::PathBuf;
use std::time::Instant;

pub(in crate::app) const UPLOAD_MAX_BYTES: u64 = 256 * 1_048_576;
pub(in crate::app) const UPLOAD_CHUNK_BYTES: u64 = 4 * 1_048_576;

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
