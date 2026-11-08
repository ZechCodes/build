use super::*;
use crate::execution::CallContext;
use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Weak};
use std::time::{Duration, Instant};

pub(in crate::app) fn dispatch(
    state: &Arc<Mutex<AppState>>,
    sender: &CallContext,
    id: Value,
    method: &str,
    params: Value,
    metadata: Value,
) -> Value {
    start(state);
    if !sender.execution_is_live() {
        return ApiError::unavailable("request authorization has ended").into_reply(id);
    }
    if state.lock().unwrap().creation_wakeup.is_none() {
        return ApiError::unavailable("creation executor could not start").into_reply(id);
    }
    let result = serde_json::from_value::<WriteMetadata>(metadata)
        .map_err(|e| ApiError::invalid_params(e.to_string()))
        .and_then(|metadata| admit(state, method, params, metadata));
    match result {
        Err(error) => error.into_reply(id),
        Ok(record) => {
            let wakeup = {
                let mut app = state.lock().unwrap();
                if !matches!(record.state.as_str(), "succeeded" | "failed") {
                    app.track_creation_workspace(&record.execution);
                    if let Err(error) = app.hold_creation_sources(&record.execution) {
                        eprintln!("restore creation sources: {error}");
                    }
                }
                app.creation_wakeup.clone()
            };
            if let Some(wakeup) = wakeup {
                let _ = wakeup.try_send(());
            }
            let reply = json!({"id":id,"ok":true,"bridge_epoch":crate::bridge_epoch::current(),"write":{"phase":"ack","state":"accepted","operation_id":record.operation_id,"revision":1,"accepted_epoch":record.accepted_epoch,"owners":record.owners},"result":record.acceptance,"outcome":record.outcome});
            reply
        }
    }
}
pub(in crate::app) fn start(state: &Arc<Mutex<AppState>>) {
    let mut app = state.lock().unwrap();
    if app.creation_wakeup.is_some() || app.store.is_none() {
        return;
    }
    let writer = app.store.as_ref().expect("store checked").writer();
    let (sender, receiver) = mpsc::sync_channel(1);
    app.creation_wakeup = Some(sender);
    drop(app);
    if let Err(error) = writer.recover_mutations() {
        eprintln!("classify interrupted writes: {error}");
    }
    match writer.pending_creations() {
        Ok(records) => {
            let mut app = state.lock().unwrap();
            for record in records {
                app.track_creation_workspace(&record.execution);
                if let Err(error) = app.hold_creation_sources(&record.execution) {
                    eprintln!("restore creation sources: {error}");
                }
            }
        }
        Err(error) => eprintln!("restore creation reservations: {error}"),
    }
    let weak = Arc::downgrade(state);
    if let Err(error) = std::thread::Builder::new()
        .name("bridge-creation".into())
        .spawn(move || worker(weak, receiver))
    {
        eprintln!("start creation executor: {error}");
        state.lock().unwrap().creation_wakeup = None;
    }
}
fn worker(state: Weak<Mutex<AppState>>, receiver: mpsc::Receiver<()>) {
    let mut retries = RetryBackoff::default();
    loop {
        let Some(app) = state.upgrade() else {
            return;
        };
        let writer = app.lock().unwrap().store.as_ref().map(|s| s.writer());
        if let Some(writer) = writer {
            if let Err(error) = reconcile_pending(&app, &writer, &mut retries, Instant::now()) {
                eprintln!("read pending creations: {error}");
            }
        }
        drop(app);
        if matches!(
            receiver.recv_timeout(Duration::from_secs(30)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ) {
            return;
        }
    }
}

#[derive(Default)]
pub(super) struct RetryBackoff {
    records: HashMap<i64, Retry>,
}

struct Retry {
    delay: Duration,
    next: Instant,
}

impl RetryBackoff {
    fn ready(&self, record: &MutationRecord, now: Instant) -> bool {
        record.state != "unknown"
            || self
                .records
                .get(&record.command_id)
                .is_none_or(|retry| now >= retry.next)
    }

    fn observe(&mut self, record: &MutationRecord, now: Instant) {
        if record.state != "unknown" {
            self.records.remove(&record.command_id);
            return;
        }
        let delay = self
            .records
            .get(&record.command_id)
            .map(|retry| (retry.delay * 2).min(Duration::from_secs(1800)))
            .unwrap_or(Duration::from_secs(30));
        self.records.insert(
            record.command_id,
            Retry {
                delay,
                next: now + delay,
            },
        );
    }
}

pub(super) fn reconcile_pending(
    app: &Arc<Mutex<AppState>>,
    writer: &StoreWriter,
    retries: &mut RetryBackoff,
    now: Instant,
) -> Result<(), String> {
    let records = writer
        .pending_creations()
        .map_err(|error| error.to_string())?;
    let pending: HashSet<_> = records.iter().map(|record| record.command_id).collect();
    retries.records.retain(|id, _| pending.contains(id));
    for record in records {
        if !retries.ready(&record, now) {
            continue;
        }
        let id = record.operation_id.clone();
        if let Err(error) = execute(app, record) {
            eprintln!("creation reconciliation: {error}");
        }
        if let Some(record) = writer
            .creation_record(PRINCIPAL, &id)
            .map_err(|error| error.to_string())?
        {
            retries.observe(&record, now);
        }
    }
    Ok(())
}
