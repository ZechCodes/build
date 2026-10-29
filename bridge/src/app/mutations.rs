//! Authenticated durable recovery reads and replay. Neither path takes AppState
//! or the writable connection; confirmation delivery bypasses ChangeBus.
use super::read_context::ReadContextSlot;
use super::AppState;
use crate::api::writes::{
    self, OperationSummary, StatusParams, StatusResult, SubscribeParams, SubscribeResult,
};
use crate::api::ApiError;
use crate::carrier::SessionSender;
use crate::store::{MutationRecord, ReadConnection, ReadPool, ReadTransaction, StoreError};
use crate::transport::Frame;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

// The current paired bridge has one authenticated device user. Session UUIDs
// and client hints are transport identities, never durable actor identities.
const DEVICE_USER: &str = "device_user";
const PAGE_OVERHEAD: usize = 1024;

#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::app) struct MutationAuthority {
    owners: HashSet<(String, String)>,
}

impl MutationAuthority {
    pub(in crate::app) fn capture(app: &AppState) -> Self {
        let mut owners = HashSet::new();
        owners.extend(
            app.projects
                .iter()
                .map(|p| ("project".into(), p.id.clone())),
        );
        owners.extend(
            app.workspaces
                .iter()
                .map(|w| ("workspace".into(), w.id.clone())),
        );
        owners.extend(app.plans.keys().map(|id| ("plan".into(), id.clone())));
        owners.extend(app.runs.keys().map(|id| ("run".into(), id.clone())));
        Self { owners }
    }

    fn permits(&self, operation: &MutationRecord) -> bool {
        // These existing owner identities have no reset generation. Future
        // enabled methods must publish their own generation-aware authority.
        !operation.owners.is_empty()
            && operation.owners.iter().all(|owner| {
                owner.generation == 0
                    && self
                        .owners
                        .contains(&(owner.kind.clone(), owner.id.clone()))
            })
            && operation.scope.as_object().is_some_and(|scope| {
                scope.iter().all(|(key, value)| {
                    key == "project_id"
                        && value
                            .as_str()
                            .is_some_and(|id| self.owners.contains(&("project".into(), id.into())))
                })
            })
    }
}

#[derive(Clone)]
pub(in crate::app) struct MutationService {
    inner: Arc<Service>,
}

struct Service {
    context: ReadContextSlot,
    pool: Option<ReadPool>,
    wakeup: Arc<tokio::sync::Notify>,
    subscriptions: Mutex<HashMap<(u64, Option<u64>), Subscription>>,
    worker: Mutex<bool>,
    #[cfg(test)]
    after_snapshot: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

#[derive(Clone)]
struct Subscription {
    sender: SessionSender,
    ids: HashSet<String>,
    cursor: u64,
    identity: Arc<()>,
    order: u64,
}

impl MutationService {
    pub(in crate::app) fn new(app: &AppState) -> Self {
        Self {
            inner: Arc::new(Service {
                context: Arc::clone(&app.read_context),
                pool: app.store.as_ref().map(crate::store::Store::read_pool),
                wakeup: app
                    .store
                    .as_ref()
                    .map(crate::store::Store::mutation_wakeup)
                    .unwrap_or_default(),
                subscriptions: Mutex::new(HashMap::new()),
                worker: Mutex::new(false),
                #[cfg(test)]
                after_snapshot: Mutex::new(None),
            }),
        }
    }

    pub(in crate::app) fn call(
        &self,
        sender: SessionSender,
        frame: Frame,
        mut connection: Option<ReadConnection>,
    ) -> Value {
        let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
        let result = self.read(sender, &frame.payload, connection.as_mut());
        crate::api::reply(id, result)
    }

    fn read(
        &self,
        sender: SessionSender,
        payload: &Value,
        connection: Option<&mut ReadConnection>,
    ) -> Result<Value, ApiError> {
        let method = payload["method"].as_str().unwrap_or("");
        let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
        let params = crate::api::v1::normalize_read_params(method, &params)?;
        let connection = connection
            .ok_or_else(|| ApiError::unavailable("write recovery needs a durable store"))?;
        if !sender.session_is_open() {
            return Err(ApiError::unavailable("session ended"));
        }
        let result = if method == "writes.subscribe" {
            static NEXT_ORDER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let order = if sender.request_order() == 0 {
                NEXT_ORDER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            } else {
                sender.request_order()
            };
            let params: SubscribeParams = serde_json::from_value(params).map_err(invalid_params)?;
            self.start_worker()?;
            let (result, cursor) = connection
                .read(|read| {
                    let cursor = read.mutation_cursor(DEVICE_USER).map_err(store_error)?;
                    let result = self.page(read, DEVICE_USER, &params.operation_ids, None)?;
                    Ok::<_, ApiError>((result, cursor))
                })
                .map_err(store_error)??;
            #[cfg(test)]
            let hook = self.inner.after_snapshot.lock().unwrap().take();
            #[cfg(test)]
            if let Some(hook) = hook {
                hook();
            }
            let subscription = Subscription {
                sender: sender.event_context(),
                ids: params.operation_ids.into_iter().collect(),
                cursor,
                identity: Arc::new(()),
                order,
            };
            let mut subscriptions = self.inner.subscriptions.lock().unwrap();
            if !sender.read_is_live() || !sender.events_are_live() {
                return Err(ApiError::unavailable("subscription carrier ended"));
            }
            let key = sender.read_scope();
            if subscriptions
                .get(&key)
                .is_none_or(|previous| previous.order < order)
            {
                subscriptions.insert(key, subscription);
            }
            drop(subscriptions);
            self.inner.wakeup.notify_one();
            serde_json::to_value(SubscribeResult {
                journal_incarnation: result.journal_incarnation,
                cursor: cursor.to_string(),
                operations: result.operations,
                next_cursor: result.next_cursor,
            })
        } else {
            let params: StatusParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = connection
                .read(|read| {
                    self.page(
                        read,
                        DEVICE_USER,
                        &params.operation_ids,
                        params.cursor.as_deref(),
                    )
                })
                .map_err(store_error)??;
            serde_json::to_value(result)
        }
        .map_err(internal_error)?;
        crate::api::v1::check_read_result(method, result)
    }

    fn page(
        &self,
        read: &ReadTransaction<'_>,
        principal: &str,
        ids: &[String],
        cursor: Option<&str>,
    ) -> Result<StatusResult, ApiError> {
        let incarnation = read.journal_incarnation().map_err(store_error)?;
        let binding = page_binding(principal, &incarnation, ids);
        let start = page_offset(cursor, &binding, ids.len())?;
        let authority = self
            .inner
            .context
            .load_full()
            .expect("initialized write read context");
        let mut operations = Vec::new();
        let mut bytes = PAGE_OVERHEAD;
        let mut next_cursor = None;
        for (offset, id) in ids.iter().enumerate().skip(start) {
            // Load at most one bounded record beyond the response budget.
            let record = read
                .mutation_status(principal, std::slice::from_ref(id))
                .map_err(store_error)?
                .pop()
                .flatten();
            let summary = match record.filter(|record| self.authorized(record)) {
                Some(record) => summary(&record)?,
                None => OperationSummary::NotFound {
                    operation_id: id.clone(),
                },
            };
            let encoded = serde_json::to_vec(&summary).map_err(internal_error)?.len() + 1;
            if bytes + encoded > writes::MAX_STATUS_BYTES {
                if operations.is_empty() {
                    return Err(ApiError::unavailable(
                        "write summary exceeds the recovery page limit",
                    ));
                }
                next_cursor = Some(format!("{binding}:{offset}"));
                break;
            }
            bytes += encoded;
            operations.push(summary);
        }
        if !self
            .inner
            .context
            .load_full()
            .is_some_and(|latest| latest.mutations == authority.mutations)
        {
            return Err(ApiError::unavailable(
                "write scope changed during recovery read",
            ));
        }
        Ok(StatusResult {
            journal_incarnation: incarnation,
            operations,
            next_cursor,
        })
    }

    fn authorized(&self, record: &MutationRecord) -> bool {
        self.inner
            .context
            .load_full()
            .is_some_and(|context| context.mutations.permits(record))
    }

    fn start_worker(&self) -> Result<(), ApiError> {
        let mut started = self.inner.worker.lock().unwrap();
        if *started {
            return Ok(());
        }
        // A single dedicated replay thread owns independent readers for all
        // subscriptions. It never consumes application or SQL request workers.
        let weak = Arc::downgrade(&self.inner);
        let wakeup = Arc::clone(&self.inner.wakeup);
        std::thread::Builder::new()
            .name("bridge-write-replay".into())
            .spawn(move || replay_worker(weak, wakeup))
            .map_err(internal_error)?;
        *started = true;
        Ok(())
    }
}

fn page_binding(principal: &str, incarnation: &str, ids: &[String]) -> String {
    let input = serde_json::to_vec(&(principal, incarnation, ids)).expect("string list serializes");
    format!("{:x}", Sha256::digest(input))
}

fn page_offset(cursor: Option<&str>, binding: &str, len: usize) -> Result<usize, ApiError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let (bound, offset) = cursor
        .split_once(':')
        .ok_or_else(|| ApiError::invalid_params("invalid write status cursor"))?;
    let offset = offset.parse::<usize>().map_err(invalid_params)?;
    if bound != binding || offset >= len {
        return Err(ApiError::invalid_params(
            "write status cursor does not match this operation set",
        ));
    }
    Ok(offset)
}

fn summary(record: &MutationRecord) -> Result<OperationSummary, ApiError> {
    let mut value = if record.expired {
        json!({"state":"result_expired","outcome":record.state,"effect":record.outcome.as_ref().and_then(|outcome| outcome.get("effect"))})
    } else if matches!(
        record.state.as_str(),
        "accepted" | "executing" | "committed"
    ) {
        let owners: Vec<_> = record.owners.iter().map(|owner| json!({"kind":owner.kind,"id":owner.id,"generation":owner.generation,"sequence":owner.sequence})).collect();
        json!({"state": if record.state == "accepted" {"accepted"} else {"executing"}, "ack":{"phase":"ack","state":"accepted","operation_id":record.operation_id,"revision":1,"accepted_epoch":record.accepted_epoch,"owners":owners},"result":record.acceptance})
    } else {
        record
            .outcome
            .clone()
            .ok_or_else(|| ApiError::unavailable("write outcome not yet available"))?
    };
    value["operation_id"] = json!(record.operation_id);
    value["revision"] = json!(record.revision);
    serde_json::from_value(value).map_err(internal_error)
}

fn confirmation(record: &MutationRecord) -> Result<Value, ApiError> {
    if record.expired {
        return Err(ApiError::unavailable(
            "write confirmation result expired; query status",
        ));
    }
    let mut outcome = record
        .outcome
        .clone()
        .ok_or_else(|| ApiError::unavailable("write outcome not yet available"))?;
    outcome["type"] = json!("writes.confirmed");
    outcome["bridge_epoch"] = json!(crate::bridge_epoch::current());
    outcome["operation_id"] = json!(record.operation_id);
    outcome["revision"] = json!(record.revision);
    let checked: writes::WriteConfirmation =
        serde_json::from_value(outcome).map_err(internal_error)?;
    serde_json::to_value(checked).map_err(internal_error)
}

fn replay_worker(service: Weak<Service>, wakeup: Arc<tokio::sync::Notify>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("write replay runtime");
    loop {
        let notified = wakeup.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let Some(inner) = service.upgrade() else {
            break;
        };
        let subscriptions: Vec<_> = inner
            .subscriptions
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        let service_handle = MutationService { inner };
        for subscription in subscriptions {
            runtime.block_on(replay(&service_handle, subscription));
        }
        drop(service_handle);
        runtime.block_on(async { tokio::select! { _ = notified => (), _ = tokio::time::sleep(Duration::from_millis(100)) => () } });
    }
}

async fn replay(service: &MutationService, subscription: Subscription) {
    if !subscription.sender.read_is_live() {
        remove_subscription(service, &subscription);
        return;
    }
    let Some(pool) = &service.inner.pool else {
        return;
    };
    let mut connection = pool.acquire().await;
    let result = connection.read(|read| read.mutation_outbox(DEVICE_USER, subscription.cursor, 1));
    drop(connection);
    let Ok(Ok(entries)) = result else {
        return;
    };
    for entry in entries {
        // Registration/replacement and delivery share this short gate. An old
        // replay that waited for SQL cannot emit after its set was replaced.
        let mut subscriptions = service.inner.subscriptions.lock().unwrap();
        let key = subscription.sender.read_scope();
        let Some(current) = subscriptions
            .get_mut(&key)
            .filter(|current| Arc::ptr_eq(&current.identity, &subscription.identity))
        else {
            return;
        };
        if subscription.ids.contains(&entry.operation.operation_id)
            && service.authorized(&entry.operation)
        {
            if let Ok(payload) = confirmation(&entry.operation) {
                if !subscription.sender.push_scoped(payload) {
                    subscriptions.remove(&key);
                    return;
                }
            }
        }
        current.cursor = entry.cursor;
        drop(subscriptions);
        service.inner.wakeup.notify_one();
    }
}

fn remove_subscription(service: &MutationService, subscription: &Subscription) {
    let mut subscriptions = service.inner.subscriptions.lock().unwrap();
    if subscriptions
        .get(&subscription.sender.read_scope())
        .is_some_and(|current| Arc::ptr_eq(&current.identity, &subscription.identity))
    {
        subscriptions.remove(&subscription.sender.read_scope());
    }
}

fn store_error(error: StoreError) -> ApiError {
    ApiError::unavailable(error.to_string())
}
fn invalid_params(error: impl std::fmt::Display) -> ApiError {
    ApiError::invalid_params(error.to_string())
}
fn internal_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::classify(error.to_string())
}

#[cfg(test)]
mod tests;
