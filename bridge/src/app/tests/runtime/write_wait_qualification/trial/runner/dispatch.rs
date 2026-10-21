//! Open-loop offers, actual credits and real execution task ownership.
use super::*;
fn queue_index(slot: &Slot) -> usize {
    match slot.traffic {
        Traffic::Write => slot.caller,
        Traffic::Read => CALLERS + slot.caller,
        Traffic::Exempt => 2 * CALLERS + slot.caller,
    }
}
fn credit_bound(index: usize) -> usize {
    if index < CALLERS {
        WRITE_CREDITS
    } else if index < 2 * CALLERS {
        if index - CALLERS < 4 {
            SPA_READ_CREDITS
        } else {
            MCP_SOCKET_CREDITS
        }
    } else {
        EXEMPT_CREDITS_PER_SPA
    }
}
impl RunState<'_> {
    pub(super) fn offer_due(&mut self, now: Instant) {
        while self.next < self.plan.len()
            && self.start + Duration::from_micros(self.plan[self.next].due_us) <= now
        {
            let slot = self.plan[self.next].clone();
            if let Err(error) = self.observe.stage(&slot.original_id, Stage::Offered, now) {
                self.observe
                    .fail(Some(slot.original_id.clone()), "offer", error);
                self.failed = true;
                break;
            }
            self.backend.caller_offered(&slot.original_id, now);
            self.pending[queue_index(&slot)].push_back(slot);
            self.next += 1;
        }
    }
    pub(super) fn dispatch_ready(&mut self) {
        for index in 0..self.pending.len() {
            while self.active[index] < credit_bound(index) {
                let Some(slot) = self.pending[index].front() else {
                    break;
                };
                if !self.eligible(slot) {
                    break;
                }
                if !self.dispatch_one(index) {
                    break;
                }
            }
        }
    }
    fn eligible(&self, slot: &Slot) -> bool {
        if slot.route == Route::AgentMcp && self.sockets[slot.caller] == MCP_SOCKET_CREDITS {
            return false;
        }
        self.recipes[&slot.original_id]
            .predecessor
            .as_ref()
            .is_none_or(|before| {
                let ledger = self.observe.ledger.lock().unwrap();
                ledger.rows[before]
                    .milestones_us
                    .contains_key(&Stage::Completed)
                    && (ledger.rows[before].error.is_none()
                        || (self.control && self.control_effects.contains(before)))
            })
    }
    fn dispatch_one(&mut self, index: usize) -> bool {
        let slot = self.pending[index].pop_front().unwrap();
        let recipe = self.recipes[&slot.original_id].clone();
        if let Err(error) =
            self.observe
                .stage(&slot.original_id, Stage::DispatchStarted, Instant::now())
        {
            self.observe
                .fail(Some(slot.original_id.clone()), "dispatch", error);
            self.pending[index].push_front(slot);
            self.failed = true;
            return false;
        }
        self.active[index] += 1;
        let socket_caller = if slot.route == Route::AgentMcp {
            self.sockets[slot.caller] += 1;
            slot.caller
        } else {
            usize::MAX
        };
        let id = slot.original_id.clone();
        let original_id = id.clone();
        let executing = Arc::clone(&self.backend);
        let observing = self.observe.clone();
        let finished = self.done_tx.clone();
        let control = self.control;
        let task = tokio::spawn(async move {
            // Panics retain unfinished attribution until independently detected.
            let result = if control {
                executing.execute_control(slot, recipe, observing).await
            } else {
                executing
                    .execute(slot, recipe, observing)
                    .await
                    .map(ControlExecution::Verified)
            };
            let _ = finished.send(ExecutionEvent::Finished(Finished {
                original_id,
                result,
            }));
        });
        self.running.insert(
            id,
            RunningExecution {
                credits: credits::CallerCredits::acquired(
                    index,
                    (socket_caller != usize::MAX).then_some(socket_caller),
                ),
                task,
            },
        );
        true
    }
    pub(super) async fn collect_done(&mut self) {
        while let Ok(event) = self.done_rx.try_recv() {
            match event {
                ExecutionEvent::DecodedReply {
                    original_id,
                    actual_at,
                } => {
                    if let Err(error) = apply_decoded(
                        &mut self.running,
                        &mut self.active,
                        &mut self.sockets,
                        &self.observe,
                        &original_id,
                        actual_at,
                    ) {
                        self.observe
                            .fail(Some(original_id), "decoded_reply_credit", error);
                    }
                }
                ExecutionEvent::Finished(done) => self.collect_finished(done).await,
            }
        }
    }
    async fn collect_finished(&mut self, done: Finished) {
        let task = match take_finished(
            &mut self.running,
            &mut self.active,
            &mut self.sockets,
            &done.original_id,
        ) {
            Ok(task) => task,
            Err(error) => {
                self.observe
                    .fail(Some(done.original_id), "duplicate_completion", error);
                return;
            }
        };
        if let Err(error) = task.await {
            self.observe
                .fail(Some(done.original_id.clone()), "join", error.to_string());
        }
        self.record_done(done);
    }
    fn record_done(&mut self, done: Finished) {
        match done.result {
            Ok(ControlExecution::Verified(proof)) => {
                self.observe.completed(&done.original_id, Ok(proof))
            }
            Ok(ControlExecution::ProofGap(gap)) if self.control => {
                if gap.original_id != done.original_id {
                    self.observe.completed(
                        &done.original_id,
                        Err("control gap original mismatch".into()),
                    );
                    return;
                }
                if gap.actual_effect_verified {
                    self.control_effects.insert(done.original_id.clone());
                }
                self.observe.control_gap(&done.original_id, &gap.detail);
                self.control_gaps.push(gap);
            }
            Ok(ControlExecution::ProofGap(_)) => self.observe.completed(
                &done.original_id,
                Err("control-only proof gap reached normal runner".into()),
            ),
            Err(error) => self.observe.completed(&done.original_id, Err(error)),
        }
    }
    pub(super) fn detect_crashed(&self) {
        let crashed = self
            .running
            .iter()
            .filter(|(_, execution)| execution.task.is_finished())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in crashed {
            if self.running[&id].task.is_finished() && self.done_rx.is_empty() {
                self.observe.fail(
                    Some(id),
                    "task_finished_without_collected_result",
                    "task may have panicked; original retained pending drain".into(),
                );
            }
        }
    }
}

/// Real coordinator event reducer. Credit release leaves every execution owned.
fn apply_decoded(
    running: &mut Running,
    active: &mut [usize; 3 * CALLERS],
    sockets: &mut [usize; CALLERS],
    observe: &Observation,
    original_id: &str,
    actual_at: Instant,
) -> Result<(), String> {
    let execution = running
        .get_mut(original_id)
        .ok_or_else(|| format!("decoded reply has no running original {original_id}"))?;
    observe.stage(original_id, Stage::DecodedReplyObserved, actual_at)?;
    execution.credits.decoded(active, sockets)?;
    let releases_credit = {
        let ledger = observe.ledger.lock().unwrap();
        let slot = &ledger.rows[original_id].slot;
        slot.traffic == Traffic::Write || slot.route == Route::AgentMcp
    };
    if releases_credit {
        observe.stage(original_id, Stage::CallerCreditReleased, Instant::now())?;
    }
    Ok(())
}
fn take_finished(
    running: &mut Running,
    active: &mut [usize; 3 * CALLERS],
    sockets: &mut [usize; CALLERS],
    original_id: &str,
) -> Result<JoinHandle<()>, String> {
    let execution = running
        .remove(original_id)
        .ok_or_else(|| format!("completion has no running original {original_id}"))?;
    execution.credits.finished(active, sockets);
    Ok(execution.task)
}
#[cfg(test)]
#[path = "dispatch/event_tests.rs"]
mod event_tests;
