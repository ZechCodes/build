//! ReadPool-only head/cursor reads. The execution adapter supplies its epoch.
use super::{cursor, retention, schema, ResyncReason, Scope, Sequence, SyncError};
use crate::store::{ReadConnection, ReadPool, ReadTransaction};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Head {
    pub incarnation: String,
    pub head: Sequence,
    pub retention_floor: Sequence,
    pub bridge_epoch: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum HeadResult {
    #[serde(rename = "ok")]
    Ready(Head),
    #[serde(rename = "resync_required")]
    ResyncRequired {
        reason: ResyncReason,
        incarnation: String,
        head: Sequence,
        bridge_epoch: String,
    },
}

impl Head {
    fn resync(self, reason: ResyncReason) -> HeadResult {
        HeadResult::ResyncRequired {
            reason,
            incarnation: self.incarnation,
            head: self.head,
            bridge_epoch: self.bridge_epoch,
        }
    }
}

/// Class quota stays with the reader through cancellation and blocking work.
pub struct SyncReadConnection {
    connection: ReadConnection,
}
impl ReadPool {
    pub async fn acquire_sync(&self) -> SyncReadConnection {
        SyncReadConnection {
            connection: self.acquire_sync_connection().await,
        }
    }
}
impl SyncReadConnection {
    /// Run one bounded, typed recovery read while retaining SyncRecovery quota.
    pub fn read<T>(
        &mut self,
        work: impl FnOnce(&ReadTransaction<'_>) -> T,
    ) -> Result<T, crate::store::StoreError> {
        self.connection.read(work)
    }
    pub fn head(
        &mut self,
        scope: &Scope,
        cursor: Option<&str>,
        epoch: &str,
        now: i64,
    ) -> Result<HeadResult, SyncError> {
        self.connection
            .read(|read| read.sync_head(scope, cursor, epoch, now))?
    }
}
impl ReadTransaction<'_> {
    /// Capture head and retention proof together. Head is observational, never
    /// a success/applied cursor for data the caller has not received.
    pub fn sync_head(
        &self,
        scope: &Scope,
        token: Option<&str>,
        epoch: &str,
        now: i64,
    ) -> Result<HeadResult, SyncError> {
        super::device::ensure_scope_ready(self.connection(), scope)?;
        let conn = self.connection();
        let (incarnation, key) = schema::identity(conn)?;
        let (head, baseline): (i64, i64) =
            conn.query_row("SELECT head,baseline FROM sync_state WHERE id=1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        let cutoff = retention::logical_floor(conn, now, retention::Policy::DEFAULT)?;
        let stream = scope.stream();
        let floor: i64 = conn
            .query_row(
                "SELECT floor FROM sync_stream_state WHERE stream_kind=?1 AND stream_key=?2",
                params![stream.kind.as_str(), stream.key],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let pending_floor: Option<i64> = conn.query_row(
            "SELECT MAX(seq) FROM sync_routes WHERE stream_kind=?1 AND stream_key=?2 AND seq<=?3",
            params![stream.kind.as_str(), stream.key, cutoff],
            |r| r.get(0),
        )?;
        let floor = floor.max(pending_floor.unwrap_or(0)).max(baseline);
        let response = Head {
            incarnation: incarnation.clone(),
            head: Sequence::new(head)?,
            retention_floor: Sequence::new(floor)?,
            bridge_epoch: epoch.into(),
        };
        if let Some(token) = token {
            return validate_cursor(response, cursor::decode(token, &incarnation, &key, scope)?);
        }
        Ok(HeadResult::Ready(response))
    }

    pub fn sync_cursor(&self, scope: &Scope, sequence: Sequence) -> Result<String, SyncError> {
        super::device::ensure_scope_ready(self.connection(), scope)?;
        let (incarnation, key) = schema::identity(self.connection())?;
        Ok(cursor::encode(&incarnation, &key, scope, sequence))
    }
}

fn validate_cursor(head: Head, position: cursor::Position) -> Result<HeadResult, SyncError> {
    match position {
        cursor::Position::Foreign => Ok(head.resync(ResyncReason::IncarnationChanged)),
        cursor::Position::Rebound(_) => Ok(head.resync(ResyncReason::ScopeRemoved)),
        cursor::Position::Retained(sequence) if sequence > head.head => {
            Err(SyncError::FutureCursor)
        }
        cursor::Position::Retained(sequence) if sequence < head.retention_floor => {
            Ok(head.resync(ResyncReason::RetentionExpired))
        }
        cursor::Position::Retained(_) => Ok(HeadResult::Ready(head)),
    }
}
