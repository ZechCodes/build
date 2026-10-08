//! Shared RPC/MCP parameter bounds; no filesystem or supplied authors.
use super::*;
use std::collections::BTreeSet;

fn invalid(message: &str) -> String {
    format!("invalid review params: {message}")
}

fn identifier(value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 200 || value.contains('\0') {
        return Err(invalid("IDs must contain 1 to 200 bytes"));
    }
    Ok(())
}

fn oid(value: &str) -> Result<(), String> {
    if value.len() != 40 || git2::Oid::from_str(value).is_err() {
        return Err(invalid("expected Git heads must be full 40-character OIDs"));
    }
    Ok(())
}

fn selections<'a>(ids: impl Iterator<Item = &'a str>, required: bool) -> Result<(), String> {
    let ids: Vec<_> = ids.collect();
    if ids.len() > 100 || (required && ids.is_empty()) {
        return Err(invalid("selections must contain 1 to 100 directories"));
    }
    if ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
        return Err(invalid("directory selections must be unique"));
    }
    ids.into_iter().try_for_each(identifier)
}

fn branch(value: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.len() > 1_024
        || !git2::Reference::is_valid_name(&crate::isolation::local_branch_ref(value))
    {
        return Err(invalid("branch must name a local Git branch"));
    }
    Ok(())
}

fn bases(values: &[ReviewBaseSelection], required: bool) -> Result<(), String> {
    selections(
        values.iter().map(|value| value.directory_id.as_str()),
        required,
    )?;
    values.iter().try_for_each(|value| branch(&value.branch))
}

impl ReviewOpenParams {
    pub fn validate(&self) -> Result<(), String> {
        identifier(&self.workspace_id)?;
        identifier(&self.request_id)?;
        if self.title.trim().is_empty()
            || self.title.len() > crate::tracker::MAX_TITLE_BYTES
            || self.title.contains(['\n', '\r', '\0'])
        {
            return Err(invalid(
                "title must be a nonempty single line of at most 200 bytes",
            ));
        }
        if self.description.len() > crate::tracker::MAX_BODY_BYTES {
            return Err(invalid("description must be at most 32000 bytes"));
        }
        if let Some(ReviewReviewer::Agent { agent_id }) = &self.reviewer {
            identifier(agent_id)?;
        }
        bases(&self.bases, false)?;
        selections(
            self.excluded_git_directory_ids.iter().map(String::as_str),
            false,
        )
    }
}

impl ReviewPushParams {
    pub fn validate(&self) -> Result<(), String> {
        identifier(&self.task_id)?;
        selections(
            self.sources
                .iter()
                .map(|source| source.directory_id.as_str()),
            true,
        )?;
        for source in &self.sources {
            oid(&source.expected_head)?;
            if let Some(received) = &source.expected_received_head {
                oid(received)?;
            }
        }
        Ok(())
    }
}

impl ReviewUpdateParams {
    pub fn validate(&self) -> Result<(), String> {
        identifier(&self.task_id)?;
        bases(&self.bases, true)
    }
}

impl ReviewMergeParams {
    pub fn validate(&self) -> Result<(), String> {
        identifier(&self.task_id)?;
        identifier(&self.snapshot_id)?;
        selections(
            self.sources
                .iter()
                .map(|source| source.directory_id.as_str()),
            true,
        )?;
        for source in &self.sources {
            oid(&source.expected_base_head)?;
            if let Some(push) = &source.push {
                identifier(&push.remote)?;
                branch(&push.branch)?;
            }
        }
        Ok(())
    }
}

impl ReviewCloseParams {
    pub fn validate(&self) -> Result<(), String> {
        identifier(&self.task_id)?;
        crate::reviews::records::review_description(&self.description).ok_or_else(|| {
            invalid("description must contain 1 to 2000 UTF-8 bytes after trimming")
        })?;
        Ok(())
    }
}

impl ReviewVersionParams {
    pub fn validate(&self) -> Result<(), String> {
        identifier(&self.task_id)
    }
}
