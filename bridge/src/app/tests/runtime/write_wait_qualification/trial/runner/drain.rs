//! Failure snapshots and bounded cleanup; accepted work is never cancelled.
use super::*;
impl RunState<'_> {
    pub(super) fn capture_failures(&mut self) {
        while let Ok(trigger) = self.watch_rx.try_recv() {
            {
                let mut ledger = self.observe.ledger.lock().unwrap();
                if ledger
                    .latched
                    .insert((trigger.original_id.clone(), trigger.kind.clone()))
                {
                    ledger.violations.push(trigger.clone());
                }
            }
            let snapshot = FailureSnapshot {
                at_us: self.observe.now_us(),
                trigger,
                pending_unsubmitted: self
                    .pending
                    .iter()
                    .flat_map(|queue| queue.iter().map(|slot| slot.original_id.clone()))
                    .collect(),
                running_originals: self.running.keys().cloned().collect(),
                not_yet_offered: self.plan[self.next..]
                    .iter()
                    .map(|slot| slot.original_id.clone())
                    .collect(),
                probe: self.probe.snapshot(),
            };
            if let Err(error) = self.sink.failure(self.name, &snapshot) {
                self.infrastructure.get_or_insert(error);
            }
            self.failures.push(snapshot);
            self.failed = true;
        }
    }
    pub(super) fn enter_drain(&mut self, now: Instant) {
        if (self.failed || now >= self.end) && self.drain_deadline.is_none() {
            self.producers.stop();
            self.drain_deadline = Some(now + CLEANUP_BOUND);
            self.at_600 = (now >= self.end).then(|| {
                counts(
                    self.observe.ledger.lock().unwrap().rows.values(),
                    Some(self.offer_window.as_micros() as u64),
                )
            });
        }
    }
    pub(super) fn obligations_quiet(&self, state: &ProbeSnapshot) -> bool {
        self.running.is_empty()
            && state.registry.is_empty()
            && state.deferred_active.is_empty()
            && state.receiver_pending.is_empty()
            && state.producer_active.is_empty()
            && self.producers.joined()
    }
    pub(super) fn close_when_quiet(&self) -> bool {
        let state = self.probe.snapshot();
        let no_pending = self.pending.iter().all(VecDeque::is_empty);
        if self.drain_deadline.is_some()
            && (self.failed || no_pending)
            && self.obligations_quiet(&state)
        {
            self.backend.request_close();
            return self.backend.closed();
        }
        false
    }
    pub(super) fn cleanup_expired(&mut self, now: Instant) -> bool {
        if self.drain_deadline.is_none_or(|deadline| now < deadline) {
            return false;
        }
        self.observe.fail(None, "cleanup_deadline", "bounded cleanup expired; accepted tasks retained, no cancellation or success substitution".into());
        self.infrastructure
            .get_or_insert("undrained or unclosed accepted work after cleanup bound".into());
        true
    }
    pub(super) fn progress(&mut self, now: Instant) {
        if now < self.next_progress {
            return;
        }
        let mut snapshot = counts(
            self.observe.ledger.lock().unwrap().rows.values(),
            Some(self.observe.now_us()),
        );
        for count in snapshot.values_mut() {
            let seconds = self.offer_window.as_secs_f64();
            count.offered_per_second = count.offered as f64 / seconds;
            count.sent_per_second = count.sent as f64 / seconds;
            count.admitted_per_second = count.admitted as f64 / seconds;
            count.completed_per_second = count.completed as f64 / seconds;
        }
        if let Err(error) = self
            .sink
            .progress(self.name, self.observe.now_us(), &snapshot)
        {
            self.observe.fail(None, "artifact_io", error.clone());
            self.infrastructure.get_or_insert(error);
        }
        self.next_progress = now + Duration::from_secs(1);
    }
    pub(super) async fn pause(&mut self, now: Instant) {
        let next_offer = self
            .plan
            .get(self.next)
            .map(|slot| self.start + Duration::from_micros(slot.due_us))
            .unwrap_or(self.end + CLEANUP_BOUND);
        let wake = if self.failed {
            now + Duration::from_millis(10)
        } else {
            next_offer
                .max(now + Duration::from_millis(1))
                .min(now + Duration::from_millis(10))
        };
        tokio::select! { biased;
            trigger = self.watch_rx.recv() => { if let Some(trigger) = trigger { let _ = self.observe.wake.send(trigger); } },
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {},
        }
    }
}
