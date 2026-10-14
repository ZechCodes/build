//! Synchronous provider-recovery checkpoints and a durable terminal outbox.
//! Restart abandons an outstanding automatic retry instead of replenishing it.

use crate::resume::disk::{self, Disk, RealDisk};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecoveryScope {
    pub entity_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub thread_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RecoveryContext {
    pub root: PathBuf,
    pub scope: RecoveryScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecoveryRecipient {
    pub scope: RecoveryScope,
    pub message_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecoveryStage {
    Pending,
    InFlight,
    Terminal,
    Resolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecoveryEpisode {
    pub scope: RecoveryScope,
    pub id: String,
    pub operation_id: Option<String>,
    pub summary: String,
    /// An absolute deadline; it cannot acquire a fresh monotonic epoch at boot.
    pub retry_due_ms: u64,
    pub stage: RecoveryStage,
    pub active: bool,
    pub needs_notice: bool,
    #[serde(default)]
    pub recipient: Option<RecoveryRecipient>,
}

pub(crate) struct RecoveryJournal {
    context: RecoveryContext,
    disk: Arc<dyn Disk>,
    current: Option<String>,
    last_notice: Option<String>,
    restored_notices: Vec<RecoveryEpisode>,
}

impl RecoveryJournal {
    pub(crate) fn new(context: RecoveryContext) -> Result<Self, String> {
        Self::on(context, Arc::new(RealDisk))
    }

    pub(crate) fn on(context: RecoveryContext, disk: Arc<dyn Disk>) -> Result<Self, String> {
        let directory = episode_directory(&context.root, &context.scope.agent_id);
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        disk.sync_dir(&context.root)
            .map_err(|error| error.to_string())?;
        disk.sync_dir(directory.parent().expect("recovery directory has a parent"))
            .map_err(|error| error.to_string())?;
        let _writes = episode_writes();
        let mut journal = Self {
            context,
            disk,
            current: None,
            last_notice: None,
            restored_notices: Vec::new(),
        };
        for mut record in episodes(&journal.context.root, &journal.context.scope)? {
            if matches!(
                record.stage,
                RecoveryStage::Pending | RecoveryStage::InFlight
            ) {
                abandon(&mut record);
                journal.write(&record)?;
            }
            if record.needs_notice {
                journal.restored_notices.push(record);
            }
        }
        Ok(journal)
    }

    pub(crate) fn checkpoint(
        &mut self,
        source: &str,
        operation_id: Option<String>,
        summary: String,
        retry_due_ms: u64,
    ) -> Result<(), String> {
        let _writes = episode_writes();
        self.checkpoint_locked(source, operation_id, summary, retry_due_ms)
    }

    fn checkpoint_locked(
        &mut self,
        source: &str,
        operation_id: Option<String>,
        summary: String,
        retry_due_ms: u64,
    ) -> Result<(), String> {
        let current = self
            .current
            .as_ref()
            .map(|_| self.require_active_record())
            .transpose()?;
        let id = current
            .filter(|record| record.stage == RecoveryStage::InFlight)
            .map(|record| record.id)
            .unwrap_or_else(|| {
                format!(
                    "{:x}",
                    Sha256::digest(format!("{}:{source}", self.context.scope.thread_id).as_bytes())
                )
            });
        if let Some(mut record) =
            read_episode(&self.context.root, &self.context.scope.agent_id, &id)?
        {
            if !self.record_is_active(&record) || record.id != id {
                return Err("provider recovery episode is no longer active".into());
            }
            if record.operation_id != operation_id || record.summary != summary {
                record.operation_id = operation_id;
                record.summary = summary;
                self.write(&record)?;
            }
            self.current = Some(id);
            return Ok(());
        }
        let record = RecoveryEpisode {
            scope: self.context.scope.clone(),
            id: id.clone(),
            operation_id,
            summary,
            retry_due_ms,
            stage: RecoveryStage::Pending,
            active: true,
            needs_notice: false,
            recipient: None,
        };
        self.write(&record)?;
        self.current = Some(id);
        Ok(())
    }

    pub(crate) fn retry_in_flight(&mut self) -> Result<(), String> {
        let _writes = episode_writes();
        let mut record = self.require_active_record()?;
        if record.stage != RecoveryStage::Pending {
            return Err("provider recovery retry allowance was already spent".into());
        }
        record.stage = RecoveryStage::InFlight;
        self.write(&record)
    }

    pub(crate) fn finish(
        &mut self,
        source: &str,
        operation_id: Option<String>,
        summary: String,
    ) -> Result<(), String> {
        let _writes = episode_writes();
        if self.current_record()?.is_none() {
            self.checkpoint_locked(source, operation_id.clone(), summary.clone(), 0)?;
        }
        let mut record = self.current_record()?.expect("checkpoint was created");
        if record.stage != RecoveryStage::Terminal {
            record.operation_id = operation_id;
            record.summary = summary;
            record.stage = RecoveryStage::Terminal;
            record.needs_notice = true;
            self.write(&record)?;
        }
        self.last_notice = Some(record.id);
        Ok(())
    }

    pub(crate) fn deactivate(&mut self) -> Result<(), String> {
        let _writes = episode_writes();
        for mut record in episodes(&self.context.root, &self.context.scope)? {
            if record.active {
                abandon(&mut record);
                record.active = false;
                self.write(&record)?;
            }
        }
        self.current = None;
        Ok(())
    }

    pub(crate) fn resolve(&mut self) -> Result<(), String> {
        let _writes = episode_writes();
        if let Some(mut record) = self.current_record()? {
            if record.stage != RecoveryStage::Terminal {
                record.active = false;
                record.stage = RecoveryStage::Resolved;
                self.write(&record)?;
                self.current = None;
            }
        }
        Ok(())
    }

    pub(crate) fn last_notice_id(&self) -> Option<String> {
        self.last_notice.clone()
    }

    pub(crate) fn has_unsettled_retry(&self) -> Result<bool, String> {
        Ok(self
            .current_record()?
            .is_some_and(|record| self.record_is_active(&record)))
    }

    pub(crate) fn active_episode_id(&self) -> Option<String> {
        self.current_record()
            .ok()
            .flatten()
            .filter(|record| self.record_is_active(record))
            .map(|record| record.id)
    }

    pub(crate) fn take_restored_notices(&mut self) -> Vec<RecoveryEpisode> {
        std::mem::take(&mut self.restored_notices)
    }

    fn current_record(&self) -> Result<Option<RecoveryEpisode>, String> {
        self.current
            .as_ref()
            .map(|id| read_episode(&self.context.root, &self.context.scope.agent_id, id))
            .transpose()
            .map(Option::flatten)
    }

    fn record_is_active(&self, record: &RecoveryEpisode) -> bool {
        record.scope == self.context.scope
            && record.active
            && matches!(
                record.stage,
                RecoveryStage::Pending | RecoveryStage::InFlight
            )
    }

    fn require_active_record(&self) -> Result<RecoveryEpisode, String> {
        self.current_record()?
            .filter(|record| {
                self.record_is_active(record) && self.current.as_deref() == Some(record.id.as_str())
            })
            .ok_or_else(|| "provider recovery episode is missing or no longer active".into())
    }

    fn write(&self, record: &RecoveryEpisode) -> Result<(), String> {
        let body = serde_json::to_string(record).map_err(|error| error.to_string())?;
        disk::replace(
            self.disk.as_ref(),
            &episode_path(&self.context.root, &record.scope.agent_id, &record.id)?,
            &body,
        )
    }
}

pub(crate) fn episodes(root: &Path, scope: &RecoveryScope) -> Result<Vec<RecoveryEpisode>, String> {
    let directory = episode_directory(root, &scope.agent_id);
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut records = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let record = read_path(&path)?;
        if &record.scope == scope {
            records.push(record);
        }
    }
    records.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(records)
}

pub(crate) fn all_episodes(root: &Path) -> Result<Vec<RecoveryEpisode>, String> {
    let entries = match std::fs::read_dir(root.join("provider-recovery")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut records = Vec::new();
    for entry in entries {
        let directory = entry.map_err(|error| error.to_string())?.path();
        for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                records.push(read_path(&path)?);
            }
        }
    }
    Ok(records)
}

pub(crate) fn read_episode(
    root: &Path,
    agent: &str,
    id: &str,
) -> Result<Option<RecoveryEpisode>, String> {
    let path = episode_path(root, agent, id)?;
    match std::fs::metadata(&path) {
        Ok(_) => read_path(&path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
pub(crate) fn save_episode(root: &Path, record: &RecoveryEpisode) -> Result<(), String> {
    let _writes = episode_writes();
    write_episode(root, record)
}

pub(crate) fn update_episode(
    root: &Path,
    agent: &str,
    id: &str,
    update: impl FnOnce(&mut RecoveryEpisode),
) -> Result<Option<RecoveryEpisode>, String> {
    let _writes = episode_writes();
    let Some(mut record) = read_episode(root, agent, id)? else {
        return Ok(None);
    };
    update(&mut record);
    write_episode(root, &record)?;
    Ok(Some(record))
}

fn write_episode(root: &Path, record: &RecoveryEpisode) -> Result<(), String> {
    let body = serde_json::to_string(record).map_err(|error| error.to_string())?;
    disk::replace(
        &RealDisk,
        &episode_path(root, &record.scope.agent_id, &record.id)?,
        &body,
    )
}

fn episode_writes() -> MutexGuard<'static, ()> {
    static WRITES: OnceLock<Mutex<()>> = OnceLock::new();
    WRITES.get_or_init(|| Mutex::new(())).lock().unwrap()
}

pub(crate) fn abandon(record: &mut RecoveryEpisode) {
    if matches!(
        record.stage,
        RecoveryStage::Pending | RecoveryStage::InFlight
    ) {
        record.summary = format!("Provider recovery was interrupted; any automatic retry was abandoned. Last provider error: {}", record.summary);
        record.stage = RecoveryStage::Terminal;
        record.needs_notice = true;
    }
}

fn read_path(path: &Path) -> Result<RecoveryEpisode, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("recovery checkpoint is not a regular file".into());
    }
    let mut raw = String::new();
    file.take(64 * 1024 + 1)
        .read_to_string(&mut raw)
        .map_err(|error| error.to_string())?;
    if raw.len() > 64 * 1024 {
        return Err("recovery checkpoint exceeds its bound".into());
    }
    serde_json::from_str(&raw).map_err(|error| format!("recovery checkpoint: {error}"))
}

fn episode_directory(root: &Path, agent: &str) -> PathBuf {
    root.join("provider-recovery")
        .join(format!("{:x}", Sha256::digest(agent.as_bytes())))
}

fn episode_path(root: &Path, agent: &str, id: &str) -> Result<PathBuf, String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("invalid provider recovery episode id".into());
    }
    Ok(episode_directory(root, agent).join(format!("{id}.json")))
}
