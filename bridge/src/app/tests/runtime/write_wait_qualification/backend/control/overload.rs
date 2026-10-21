//! Proposed methods inside ControlBackend; normal new() has no refusal capability.
use super::*;
impl ControlBackend {
    pub fn new_overload(
        prepared: PreparedBackend,
        sink: Arc<dyn ArtifactSink>,
    ) -> Result<Self, PreparationFailure> {
        if prepared.plan.len() != 400
            || prepared
                .plan
                .iter()
                .any(|slot| slot.traffic != Traffic::Write || slot.due_us >= 1_000_000)
        {
            return Err(PreparationFailure {
                reason: "invalid exact400 overload plan".into(),
                retained: prepared,
            });
        }
        let mut backend = Self::new(prepared, sink)?;
        let audit = super::super::overload_refusals::RefusalAudit::new(
            backend
                .slots
                .values()
                .filter(|slot| slot.traffic == Traffic::Write && slot.route == Route::SpaRpc)
                .map(|slot| {
                    (
                        slot.clone(),
                        backend.recipes[&slot.original_id].method.clone(),
                    )
                }),
        )
        .expect("immutable SPA original map already validated");
        backend
            .archive
            .lock()
            .unwrap()
            .enable_overload(Arc::clone(&audit));
        backend.overload = Some(audit);
        Ok(backend)
    }
    pub(super) async fn overload_refusal(
        &self,
        prepared: &PreparedBackend,
        slot: &Slot,
        recipe: &Recipe,
        reply: &ReplyProof,
        observe: &Observation,
    ) -> Result<Option<ControlExecution>, String> {
        if self.overload.is_none()
            || slot.route != Route::SpaRpc
            || reply.reply["error_code"] != "busy"
            || reply.reply["retryable"] != true
        {
            return Ok(None);
        }
        let mut evidence_errors = Vec::new();
        // Retain the actual acceptance vector even when the raw sink fails.
        let acceptance_records = match self.saved_evidence.archive.take_rejected(&slot.original_id)
        {
            Ok(rows) => Some(
                rows.into_iter()
                    .map(|row| serde_json::to_value(row).expect("accepted record serializes"))
                    .collect(),
            ),
            Err(error) => {
                evidence_errors.push(error);
                None
            }
        };
        let artifact = match self.sink.raw_response(
            prepared.name,
            &slot.original_id,
            &recipe.method,
            true,
            &reply.reply,
        ) {
            Ok(Some(path)) => path,
            Ok(None) => {
                evidence_errors.push("actual busy raw-response artifact missing".into());
                String::new()
            }
            Err(error) => {
                evidence_errors.push(format!("actual busy raw response: {error}"));
                String::new()
            }
        };
        // The wire can decode just before the carrier performs its final
        // native cancellation. Wait only within the original 100 ms deadline;
        // never reset it and never manufacture a missing terminal snapshot.
        let deadline = observe.original_due(slot) + std::time::Duration::from_millis(100);
        let native_terminal_rows = terminal_refusal_rows(
            self.overload.as_ref().expect("overload only"),
            &slot.original_id,
            deadline,
        )
        .await;
        let mut refusal = ControlRefusal {
            original_id: slot.original_id.clone(),
            method: recipe.method.clone(),
            raw_response_artifact: artifact,
            sent_to_decoded_final_ns: reply
                .final_reply_at
                .saturating_duration_since(reply.sent_at)
                .as_nanos()
                .min(u64::MAX as u128) as u64,
            intended_to_decoded_final_ns: reply
                .final_reply_at
                .saturating_duration_since(observe.original_due(slot))
                .as_nanos()
                .min(u64::MAX as u128) as u64,
            decoded_response: reply.reply.clone(),
            native_terminal_rows,
            acceptance_records,
            evidence_errors,
            pending_pre_durable_verification: true,
        };
        if let Err(error) = self.overload.as_ref().expect("overload only").candidate(
            slot,
            &recipe.method,
            reply,
            refusal.clone(),
            observe.original_due(slot),
        ) {
            refusal.evidence_errors.push(error);
        }
        Ok(Some(ControlExecution::Refused(refusal)))
    }
}

async fn terminal_refusal_rows(
    audit: &super::super::overload_refusals::RefusalAudit,
    original: &str,
    deadline: Instant,
) -> Vec<serde_json::Value> {
    loop {
        let anchor = Instant::now();
        let rows = audit.terminal_rows(original);
        if rows.iter().any(|row| row.completed_at.is_some()) || Instant::now() >= deadline {
            return rows
                .iter()
                .map(|row| super::super::native_archive::native_row(row, anchor))
                .collect();
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}
