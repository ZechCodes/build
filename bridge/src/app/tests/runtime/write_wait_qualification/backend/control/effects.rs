//! Actual responses and native oracles; missing state proof stays explicit RED.
use super::super::super::common_requests::{ExpectedEffect, SavedEffect};
use super::*;
use crate::app::tests::runtime::write_wait_qualification::read_recipes::FullReadOutcome;

impl ControlBackend {
    pub(super) async fn write_actual(
        &self,
        prepared: &PreparedBackend,
        slot: &Slot,
        recipe: &Recipe,
        observe: &Observation,
    ) -> Result<ControlExecution, String> {
        let result = self
            .prove_common_write(prepared, slot, recipe, observe)
            .await;
        // take_rejected records its own distinct pending refusal outcome.
        // Never label a refused original as a verified saved mutation.
        if !matches!(&result, Ok(ControlExecution::Refused(_))) {
            self.saved_evidence.archive.outcome(
                &slot.original_id,
                result.as_ref().map(|_| ()).map_err(Clone::clone),
            );
        }
        result
    }
    async fn prove_common_write(
        &self,
        prepared: &PreparedBackend,
        slot: &Slot,
        recipe: &Recipe,
        observe: &Observation,
    ) -> Result<ControlExecution, String> {
        let request = prepared
            .common
            .get(&slot.original_id)
            .ok_or("actual common request missing")?;
        let started_ms = wall_ms()?;
        let dispatched = prepared.dispatch_write(slot).await;
        let decoded_ms = wall_ms();
        let endpoints = match &dispatched {
            Ok(reply) => super::super::transport::record_reply(slot, reply, observe),
            Err(error) => Err(error.clone()),
        };
        if let Ok(reply) = &dispatched {
            if let Some(refusal) = self
                .overload_refusal(prepared, slot, recipe, reply, observe)
                .await?
            {
                endpoints?;
                return Ok(refusal);
            }
        }
        // Existing immutable intended+5s bound; no new deadline at wait time.
        let deadline = tokio::time::Instant::from_std(
            *self.start.get().ok_or("actual control start missing")?
                + std::time::Duration::from_micros(slot.due_us)
                + std::time::Duration::from_secs(5),
        );
        let completion = match &dispatched {
            Ok(_) => self.completed.wait(&slot.original_id, deadline).await,
            Err(error) => Err(format!(
                "actual dispatch failed before completion proof: {error}"
            )),
        };
        // Retain the whole vector even after failed dispatch/completion. Later
        // arrivals remain in the same collector for the final native-quiet audit.
        let records = self.saved_evidence.archive.take(request);
        let reply = dispatched?;
        let witness = async {
            endpoints?;
            let completion = completion.as_ref().map_err(Clone::clone)?;
            let records = records.as_ref().map_err(Clone::clone)?;
            super::super::saved_effects::check_original_complete(request, &reply, completion)?;
            if let Some(saved) = actual_finite_saved(&request.effect).await? {
                request.verify_saved(&reply.reply, saved)
            } else {
                self.saved_evidence
                    .verify(
                        request,
                        &reply,
                        super::super::saved_effects::InvocationWindow {
                            started_ms,
                            decoded_ms: decoded_ms?,
                        },
                        completion,
                        records,
                    )
                    .await
            }
        }
        .await;
        // The ordinary response reducer still archives every decoded failure;
        // none of the async native checks substitutes a reply-success oracle.
        let evidence = verify_and_summarize_response(
            prepared.name,
            &slot.original_id,
            &recipe.method,
            &reply.reply,
            |_| witness,
            &*self.sink,
        )?;
        let completion = completion?;
        observe.stage(&slot.original_id, Stage::EffectVerified, Instant::now())?;
        let effect = EffectProof {
            boundary: "actual-native-original-saved-and-completed".into(),
            witness: evidence.oracle_witness.clone(),
            checked_at_us: observe.now_us(),
        };
        Ok(ControlExecution::Verified(ExecutionProof {
            original_id: slot.original_id.clone(),
            method: recipe.method.clone(),
            durable_operation_id: reply.reply["result"]["operation_id"]
                .as_str()
                .map(str::to_owned),
            response_evidence: evidence,
            effect: Some(effect),
            read_cohort: None,
            registry_root: Some(format!("{:?}", completion.write_id())),
            // Only the closed token after exact original/method validation sets this.
            mandatory_children_complete: true,
        }))
    }
    pub(super) async fn read_actual(
        &self,
        prepared: &PreparedBackend,
        slot: &Slot,
        recipe: &Recipe,
        observe: &Observation,
    ) -> Result<ControlExecution, String> {
        prove_native_read(prepared, slot, recipe, observe, &*self.sink).await
    }
    pub(super) async fn exempt_actual(
        &self,
        prepared: &PreparedBackend,
        slot: &Slot,
        recipe: &Recipe,
        observe: &Observation,
    ) -> Result<ControlExecution, String> {
        let (reply, witness) = prepared.dispatch_exempt(slot, observe).await?;
        let evidence = verify_and_summarize_response(
            prepared.name,
            &slot.original_id,
            &recipe.method,
            &reply.reply,
            |_| Ok(witness),
            &*self.sink,
        )?;
        observe.stage(&slot.original_id, Stage::EffectVerified, Instant::now())?;
        Ok(ControlExecution::Verified(actual_proof(
            slot, recipe, evidence, None, observe,
        )))
    }
}
async fn actual_finite_saved(effect: &ExpectedEffect) -> Result<Option<SavedEffect>, String> {
    let effect = effect.clone();
    tokio::task::spawn_blocking(move || match effect {
        ExpectedEffect::File { path, .. } => {
            let actual_bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
            Ok(Some(SavedEffect::File {
                actual_path: path,
                actual_bytes,
            }))
        }
        ExpectedEffect::GitHead { directory, .. } => {
            let repo = git2::Repository::open(&directory).map_err(|e| e.to_string())?;
            let head = repo.head().map_err(|e| e.to_string())?;
            let actual_symbolic_head = head.name().ok_or("actual HEAD not UTF8")?.to_owned();
            let actual_commit = head
                .peel_to_commit()
                .map_err(|e| e.to_string())?
                .id()
                .to_string();
            Ok(Some(SavedEffect::GitHead {
                actual_directory: directory,
                actual_symbolic_head,
                actual_commit,
            }))
        }
        _ => Ok(None),
    })
    .await
    .map_err(|error| error.to_string())?
}
fn native_read_witness(method: &str, cohort: &ReadCohort) -> OracleWitness {
    // Strong full decoded native assert_rpc_reply ran in dispatch_read before
    // this reduction. This is its actual passed outcome, not Value echo/hash.
    OracleWitness::State {
        oracle: format!("actual-native-read/{method}"),
        assertions: vec![OracleAssertion {
            field: "verified_native_read_outcome".into(),
            actual: WitnessScalar::Text(format!("{cohort:?}")),
            expected: WitnessScalar::Text(format!("{cohort:?}")),
        }],
    }
}
fn actual_proof(
    slot: &Slot,
    recipe: &Recipe,
    response_evidence: ResponseEvidence,
    read_cohort: Option<ReadCohort>,
    observe: &Observation,
) -> ExecutionProof {
    let effect = EffectProof {
        boundary: "actual-full-native-read/control-oracle".into(),
        witness: response_evidence.oracle_witness.clone(),
        checked_at_us: observe.now_us(),
    };
    ExecutionProof {
        original_id: slot.original_id.clone(),
        method: recipe.method.clone(),
        durable_operation_id: None,
        response_evidence,
        effect: Some(effect),
        read_cohort,
        registry_root: None,
        mandatory_children_complete: true,
    }
}

fn wall_ms() -> Result<i64, String> {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis(),
    )
    .map_err(|error| error.to_string())
}

/// The same native read proof boundary used by smoke and all normal recipes.
pub(in crate::app::tests::runtime::write_wait_qualification::backend) async fn prove_native_read(
    prepared: &PreparedBackend,
    slot: &Slot,
    recipe: &Recipe,
    observe: &Observation,
    sink: &dyn ArtifactSink,
) -> Result<ControlExecution, String> {
    use super::super::transport::NativeReadOutcome;
    use crate::app::tests::runtime::write_wait_qualification::read_recipes::FullReadForm;
    let native = prepared.dispatch_read(slot, observe).await?;
    let mut cohort = None;
    let response_evidence = verify_and_summarize_response(
        prepared.name,
        &slot.original_id,
        &recipe.method,
        &native.reply.reply,
        |_| {
            let actual = match native.native_outcome? {
                NativeReadOutcome::Rpc(FullReadOutcome::Ready) => ReadCohort::ReadyBounded,
                NativeReadOutcome::Rpc(FullReadOutcome::LegacyFull) => ReadCohort::LegacyUnbounded,
                NativeReadOutcome::Rpc(_) => {
                    return Err("actual native RPC read pending/refused/unavailable".into());
                }
                NativeReadOutcome::McpVerified { actual_tool, form } => {
                    if recipe.method != format!("mcp.control:{actual_tool}") {
                        return Err("actual MCP tool differs from frozen recipe".into());
                    }
                    match form {
                        FullReadForm::Ready => ReadCohort::ReadyBounded,
                        FullReadForm::LegacyFull => ReadCohort::LegacyUnbounded,
                    }
                }
            };
            if recipe.expected_read_cohort.as_ref() != Some(&actual) {
                return Err("actual native read form differs from declared typed recipe".into());
            }
            // The full decoded native oracle ran inside dispatch_read; this
            // witness records that result, never a copied expected JSON body.
            let witness = native_read_witness(&recipe.method, &actual);
            cohort = Some(actual);
            Ok(witness)
        },
        sink,
    )?;
    observe.stage(&slot.original_id, Stage::EffectVerified, Instant::now())?;
    Ok(ControlExecution::Verified(actual_proof(
        slot,
        recipe,
        response_evidence,
        cohort,
        observe,
    )))
}
