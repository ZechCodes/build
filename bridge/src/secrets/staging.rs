//! Idempotent acceptance uses the sealed-byte digest, never a value hash.
use super::{Result, SecretError, SecretRecord, SecretService};
use crate::secrets::upload::{UploadContext, UploadOpener};
use sha2::{Digest, Sha256};

impl SecretService {
    pub fn stage_upload(
        &self,
        expected: &UploadContext,
        sealed: &str,
        opener: &dyn UploadOpener,
    ) -> Result<SecretRecord> {
        self.stage_upload_inner(expected, sealed, opener, None)
    }

    pub fn stage_upload_authorized(
        &self,
        expected: &UploadContext,
        sealed: &str,
        opener: &dyn UploadOpener,
        authority: &crate::api::v1::secrets::SecretScope,
    ) -> Result<SecretRecord> {
        self.stage_upload_inner(expected, sealed, opener, Some(authority))
    }

    fn stage_upload_inner(
        &self,
        expected: &UploadContext,
        sealed: &str,
        opener: &dyn UploadOpener,
        authority: Option<&crate::api::v1::secrets::SecretScope>,
    ) -> Result<SecretRecord> {
        expected.validate()?;
        if let Some(authority) = authority {
            self.inner
                .writer
                .secrets_authorize_upload(authority, expected)?;
        }
        let digest = format!("{:x}", Sha256::digest(sealed.as_bytes()));
        if let Some(record) = self.upload_replay(expected, &digest)? {
            return Ok(record);
        }
        let job = self.job()?;
        let value = opener.open(sealed, expected)?;
        if value.as_bytes().is_empty() {
            return Err(SecretError::Invalid);
        }
        let context = expected.secret_context(format!("sec_{}", uuid::Uuid::new_v4()));
        match self.stage_with_job(
            context,
            value,
            expected.request_id.clone(),
            digest.clone(),
            job,
            authority,
        ) {
            // A concurrent identical request may win while this one encrypts.
            Err(SecretError::Conflict) => self
                .upload_replay(expected, &digest)?
                .ok_or(SecretError::Conflict),
            outcome => outcome,
        }
    }

    fn upload_replay(
        &self,
        expected: &UploadContext,
        digest: &str,
    ) -> Result<Option<SecretRecord>> {
        let record = self.inner.writer.secret_by_upload_request(
            &expected.device_id,
            &expected.creator,
            &expected.request_id,
        )?;
        if let Some(record) = &record {
            let same = record.upload_digest == digest
                && record.context == expected.secret_context(record.context.id.clone());
            if !same {
                return Err(SecretError::Conflict);
            }
        }
        Ok(record)
    }
}

#[cfg(test)]
#[path = "staging_tests.rs"]
mod tests;
