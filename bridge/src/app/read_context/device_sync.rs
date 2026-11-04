//! Lease metadata uses the writer before the independent read transaction.
use super::{api, ApiError, ReadConnection, ReadService, Value};
use crate::api::v1::sync::{ChangesParams, HeadParams, SnapshotParams};
use crate::app::tracker::sync_scopes::TaskSyncScope;
use crate::store::{
    sync::{Continuation, DeviceLease, Sequence, SyncError},
    ReadTransaction,
};
use crate::transport::Frame;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Position {
    first_seq: Sequence,
    kind: String,
    key: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Page {
    opening: String,
    lease_id: String,
    after: Position,
}

impl ReadService {
    pub(in crate::app) fn call_for_opening(
        &self,
        frame: Frame,
        connection: Option<ReadConnection>,
        opening: &str,
        epoch: &str,
    ) -> Value {
        if frame.payload["method"]
            .as_str()
            .is_some_and(|method| method.starts_with("sync."))
            && frame.payload["params"]["scope"]["kind"] == "device"
        {
            let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
            let result = self.read_device(&frame.payload, connection, opening, epoch);
            return api::reply(id, result);
        }
        self.call(frame, connection)
    }
    fn read_device(
        &self,
        payload: &Value,
        mut connection: Option<ReadConnection>,
        opening: &str,
        epoch: &str,
    ) -> Result<Value, ApiError> {
        if !self.device_ready {
            return Err(ApiError::retryable_unavailable("sync_reconciling"));
        }
        let _admission = self
            .admission
            .as_ref()
            .map(|gate| {
                gate.try_enter_read()
                    .ok_or_else(|| ApiError::busy("bridge update handoff in progress"))
            })
            .transpose()?;
        let method = payload["method"].as_str().unwrap_or("");
        let params = crate::api::v1::normalize_read_params(method, &payload["params"])?;
        let context = self.context.load_full().expect("read context initialized");
        if context.deleting_project {
            return Err(ApiError::retryable_unavailable(
                super::super::rpc::PROJECT_DELETION_IN_PROGRESS,
            ));
        }
        let now = crate::store::sync::now_ms();
        let lease = self.admit_device(method, &params, opening, epoch, now)?;
        let connection = connection
            .as_mut()
            .ok_or_else(|| ApiError::unavailable("device sync needs a durable store"))?;
        let result = connection
            .read(|read| query(read, method, &params, lease, opening, epoch, now))
            .map_err(|error| ApiError::classify(error.to_string()))?
            .map_err(device_error)?;
        let result = crate::api::v1::check_read_result(method, result)?;
        self.release_completed_device_snapshot(method, &result, opening, epoch);
        Ok(result)
    }
    fn release_completed_device_snapshot(
        &self,
        method: &str,
        result: &Value,
        opening: &str,
        epoch: &str,
    ) {
        if method != "sync.snapshot"
            || result["status"] != "ok"
            || result.get("next_page").is_some()
        {
            return;
        }
        if let (Some(writer), Some(id)) = (&self.writer, result["lease"]["id"].as_str()) {
            writer.release_device_lease_best_effort(id, opening, epoch);
        }
    }
    fn admit_device(
        &self,
        method: &str,
        params: &Value,
        opening: &str,
        epoch: &str,
        now: i64,
    ) -> Result<Option<DeviceLease>, ApiError> {
        if method != "sync.snapshot" {
            return Ok(None);
        }
        let params: SnapshotParams = serde_json::from_value(params.clone())
            .map_err(|error| ApiError::invalid_params(error.to_string()))?;
        if !(1..=500).contains(&params.limit.unwrap_or(100)) {
            return Err(ApiError::invalid_params("snapshot limit must be 1 to 500"));
        }
        if params.page.is_some() {
            return Ok(None);
        }
        self.writer
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("device sync needs a durable store"))?
            .admit_device_lease(opening, epoch, now)
            .map(Some)
            .map_err(device_error)
    }
}
fn query(
    read: &ReadTransaction<'_>,
    method: &str,
    params: &Value,
    lease: Option<DeviceLease>,
    opening: &str,
    epoch: &str,
    now: i64,
) -> Result<Value, SyncError> {
    let recovery = TaskSyncScope::device();
    match method {
        "sync.head" => {
            let params: HeadParams = decode(params)?;
            recovery.head(read, params.cursor.as_deref(), epoch, now)
        }
        "sync.snapshot" => snapshot(read, decode(params)?, lease, opening, epoch, now),
        "sync.changes" => {
            let params: ChangesParams = decode(params)?;
            recovery.changes(
                read,
                &params.cursor,
                params.continuation.as_deref(),
                params.limit.unwrap_or(100) as usize,
                epoch,
                now,
            )
        }
        _ => Err(SyncError::InvalidRequest(
            "unknown device recovery method".into(),
        )),
    }
}
fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, SyncError> {
    serde_json::from_value(value.clone())
        .map_err(|error| SyncError::InvalidRequest(error.to_string()))
}
fn snapshot(
    read: &ReadTransaction<'_>,
    params: SnapshotParams,
    lease: Option<DeviceLease>,
    opening: &str,
    epoch: &str,
    now: i64,
) -> Result<Value, SyncError> {
    let recovery = TaskSyncScope::device();
    let scope = recovery.scope();
    let (id, after) = match params.page.as_deref() {
        Some(token) => {
            match read.sync_decode_continuation::<Page>(scope, "device_snapshot", token)? {
                Continuation::Current(page) => {
                    if page.opening != opening {
                        return Err(SyncError::ScopeMismatch);
                    }
                    (page.lease_id, Some(page.after))
                }
                Continuation::Foreign(_) | Continuation::Rebound(_) => {
                    return reset(read, epoch, "incarnation_changed")
                }
            }
        }
        None => (
            lease
                .ok_or_else(|| SyncError::Unavailable("missing device lease admission".into()))?
                .id,
            None,
        ),
    };
    let after = after.as_ref().map(|after| {
        (
            after.first_seq.get(),
            after.kind.as_str(),
            after.key.as_str(),
        )
    });
    let mut answer =
        read.sync_device_snapshot(&id, after, params.limit.unwrap_or(100) as usize, epoch, now)?;
    if answer["status"] != "ok" {
        return reset(
            read,
            epoch,
            answer["reason"].as_str().unwrap_or("lease_expired"),
        );
    }
    answer["scope"] = json!({"kind":"device"});
    let watermark: Sequence = decode(&answer["watermark"])?;
    answer["cursor"] = json!(read.sync_cursor(scope, watermark)?);
    let next = answer
        .as_object_mut()
        .expect("snapshot object")
        .remove("next");
    if let Some(next) = next {
        let page = Page {
            opening: opening.into(),
            lease_id: id,
            after: decode(&next)?,
        };
        answer["next_page"] =
            json!(read.sync_encode_continuation(scope, "device_snapshot", page)?);
    }
    if serde_json::to_vec(&answer)
        .map_err(|error| SyncError::Unavailable(error.to_string()))?
        .len()
        > crate::store::sync::MAX_RECOVERY_BYTES
    {
        return reset(read, epoch, "over_budget");
    }
    if answer["lease"]["expires_at_ms"]
        .as_i64()
        .is_none_or(|expires| expires <= crate::store::sync::now_ms())
    {
        return reset(read, epoch, "lease_expired");
    }
    Ok(answer)
}
fn reset(read: &ReadTransaction<'_>, epoch: &str, reason: &str) -> Result<Value, SyncError> {
    let mut result =
        TaskSyncScope::device().head(read, None, epoch, crate::store::sync::now_ms())?;
    result["status"] = json!("resync_required");
    result["reason"] = json!(reason);
    result
        .as_object_mut()
        .expect("head object")
        .remove("retention_floor");
    Ok(result)
}
fn device_error(error: SyncError) -> ApiError {
    match error {
        SyncError::Unavailable(message) => ApiError::retryable_unavailable(message),
        other => super::sync_error(other),
    }
}
