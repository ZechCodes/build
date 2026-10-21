use super::*;

fn require(
    transaction: &Transaction<'_>,
    principal: &str,
    id: &str,
) -> Result<MutationRecord, StoreError> {
    load(transaction, principal, id)?.ok_or_else(|| invalid("unknown command"))
}
fn validate_outcome(
    record: &MutationRecord,
    state: &str,
    outcome: Value,
) -> Result<Value, StoreError> {
    let mut outcome = outcome;
    outcome["state"] = json!(state);
    if outcome.get("refresh").is_none() {
        outcome["refresh"] = json!(record
            .owners
            .iter()
            .map(|owner| { json!({"kind":owner.kind,"id":owner.id}) })
            .collect::<Vec<_>>());
    }
    if state == "succeeded" && outcome.get("published").is_none() {
        outcome["published"] = json!([]);
    }
    if state == "succeeded" && outcome.get("result").is_none() {
        return Err(invalid("successful outcome requires a result"));
    }
    let typed: crate::api::writes::ConfirmationOutcome<Value> = serde_json::from_value(outcome)
        .map_err(|error| invalid(&format!("invalid terminal outcome: {error}")))?;
    let outcome = json!(typed);
    if outcome.to_string().len() > 256 * 1024 {
        return Err(invalid("result byte limit exceeded"));
    }
    Ok(outcome)
}
fn terminal(
    transaction: &Transaction<'_>,
    record: &mut MutationRecord,
    state: &str,
    outcome: Value,
) -> Result<(), StoreError> {
    let outcome = validate_outcome(record, state, outcome)?;
    record.state = state.into();
    record.revision += 1;
    record.outcome = Some(outcome);
    if !blocks(record) {
        transaction.execute(
            "UPDATE mutation_owner_order SET active=0 WHERE command_id=?1",
            [record.command_id],
        )?;
    }
    save(transaction, record)?;
    transaction.execute(
        "UPDATE mutation_commands SET terminal_at=unixepoch() WHERE command_id=?1",
        [record.command_id],
    )?;
    transaction.execute(
        "INSERT OR IGNORE INTO mutation_outbox(principal,command_id,revision) VALUES(?1,?2,?3)",
        params![record.principal, record.command_id, record.revision as i64],
    )?;
    Ok(())
}
fn blocks(record: &MutationRecord) -> bool {
    match record.state.as_str() {
        "succeeded" => false,
        "failed" => {
            record
                .outcome
                .as_ref()
                .and_then(|value| value.get("effect"))
                .and_then(Value::as_str)
                != Some("unapplied")
        }
        _ => true,
    }
}
fn owner_blocked(
    transaction: &Transaction<'_>,
    record: &MutationRecord,
) -> Result<bool, StoreError> {
    Ok(transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM mutation_owner_order current
         JOIN mutation_owner_order previous ON previous.kind=current.kind
          AND previous.owner_id=current.owner_id AND previous.generation=current.generation
          AND previous.sequence<current.sequence AND previous.active=1
         WHERE current.command_id=?1)",
        [record.command_id],
        |row| row.get(0),
    )?)
}
fn predecessor_state(
    transaction: &Transaction<'_>,
    record: &MutationRecord,
) -> Result<(bool, bool), StoreError> {
    let mut pending = false;
    let mut failed = false;
    for id in &record.dependencies {
        let predecessor = require(transaction, &record.principal, id)?;
        failed |= predecessor.state == "failed" && !blocks(&predecessor);
        pending |= predecessor.state != "succeeded";
    }
    Ok((pending, failed))
}
impl StoreWriter {
    /// Returns false until every owner and successful dependency is published.
    /// External execution must durably claim before crossing an effect boundary.
    pub fn claim_mutation(&self, principal: &str, id: &str) -> Result<bool, StoreError> {
        let claimed=self.transaction(|transaction| {
            let mut record=require(transaction,principal,id)?;
            if record.state!="accepted" {return Ok(false);}
            let (pending,failed)=predecessor_state(transaction,&record)?;
            if failed {
                terminal(transaction,&mut record,"failed",json!({"effect":"unapplied","error":{"code":"dependency_failed","message":"A predecessor failed","retryable":false}}))?;
                return Ok(false);
            }
            if pending || owner_blocked(transaction,&record)? {return Ok(false);}
            record.state="executing".into();record.revision+=1;save(transaction,&record)?;Ok(true)
        })?;
        self.wakeup.notify_waiters();
        Ok(claimed)
    }
    /// Canonical SQL changes and the applied marker commit together. The closure
    /// is SQL-only: external work belongs after a separately durable claim.
    pub fn commit_mutation(
        &self,
        principal: &str,
        id: &str,
        apply: impl FnOnce(&Transaction<'_>) -> Result<Value, StoreError>,
    ) -> Result<MutationRecord, StoreError> {
        self.transaction(|transaction| {
            let mut record = require(transaction, principal, id)?;
            if record.state != "executing" {
                return Err(invalid("command must be executing"));
            }
            let result = apply(transaction)?;
            if result.to_string().len() > 256 * 1024 {
                return Err(invalid("result byte limit exceeded"));
            }
            let outcome = json!({"result":result});
            // Reserve room for the mandatory confirmation envelope before
            // committing effects. The saved result can always be confirmed
            // with conservative owner refresh addresses and no extra payload.
            validate_outcome(&record, "succeeded", outcome.clone())?;
            record.outcome = Some(outcome);
            record.state = "committed".into();
            record.revision += 1;
            save(transaction, &record)?;
            Ok(record)
        })
    }
    /// Call only after publishing canonical projections and advancing observation
    /// fences. On save failure the committed record remains recoverable.
    pub fn confirm_mutation(
        &self,
        principal: &str,
        id: &str,
        publication: Value,
    ) -> Result<MutationRecord, StoreError> {
        let record = self.transaction(|transaction| {
            let mut record = require(transaction, principal, id)?;
            if record.state == "succeeded" {
                return Ok(record);
            }
            if record.state != "committed" {
                return Err(invalid("command must be committed before publication"));
            }
            let mut outcome = record.outcome.clone().unwrap_or_else(|| json!({}));
            let fields = publication
                .as_object()
                .ok_or_else(|| invalid("publication must be an object"))?;
            for (key, value) in fields {
                if !["published", "refresh"].contains(&key.as_str()) {
                    return Err(invalid("unexpected publication field"));
                }
                outcome[key] = value.clone();
            }
            if outcome.to_string().len() > 256 * 1024 {
                return Err(invalid("result byte limit exceeded"));
            }
            terminal(transaction, &mut record, "succeeded", outcome)?;
            Ok(record)
        })?;
        self.wakeup.notify_waiters();
        Ok(record)
    }
    pub fn fail_mutation(
        &self,
        principal: &str,
        id: &str,
        effect: &str,
        error: Value,
    ) -> Result<MutationRecord, StoreError> {
        if !["unapplied", "partial", "unknown"].contains(&effect)
            || error.to_string().len() > 16 * 1024
        {
            return Err(invalid("invalid failure outcome"));
        }
        let record = self.transaction(|transaction| {
            let mut record = require(transaction, principal, id)?;
            if !["accepted", "executing"].contains(&record.state.as_str()) {
                return Err(invalid("command cannot fail in this state"));
            }
            terminal(
                transaction,
                &mut record,
                "failed",
                json!({"effect":effect,"error":error}),
            )?;
            Ok(record)
        })?;
        self.wakeup.notify_waiters();
        Ok(record)
    }
    /// Recovery only classifies durable records. It never repeats an effect.
    /// Call at boot before execution/readiness; committed records must publish
    /// their canonical projections before confirm_mutation.
    pub fn recover_mutations(&self) -> Result<MutationRecovery, StoreError> {
        let recovery=self.transaction(|transaction| {
            let mut statement=transaction.prepare("SELECT record FROM mutation_commands WHERE state IN ('accepted','executing','committed') ORDER BY command_id")?;
            let records=statement.query_map([],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            drop(statement);let mut recovery=MutationRecovery::default();
            for raw in records {recover_one(transaction,decode(raw)?,&mut recovery)?;}
            transaction.execute("INSERT OR IGNORE INTO mutation_outbox(principal,command_id,revision) SELECT principal,command_id,CAST(json_extract(record,'$.revision') AS INTEGER) FROM mutation_commands WHERE state IN ('succeeded','failed')",[])?;
            Ok(recovery)
        })?;
        self.wakeup.notify_waiters();
        Ok(recovery)
    }
    /// Record a verb-specific reconciliation after evidence proves the effect.
    /// Successful outcomes require canonical publication before this call.
    pub fn reconcile_mutation(
        &self,
        principal: &str,
        id: &str,
        outcome: Value,
    ) -> Result<MutationRecord, StoreError> {
        let state = outcome
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("reconciliation state required"))?;
        if state != "succeeded"
            && !(state == "failed" && outcome.get("effect") == Some(&json!("unapplied")))
        {
            return Err(invalid("reconciliation must prove succeeded or unapplied"));
        }
        if outcome.to_string().len() > 256 * 1024 {
            return Err(invalid("result byte limit exceeded"));
        }
        let record = self.transaction(|transaction| {
            let mut record = require(transaction, principal, id)?;
            if record.state != "failed" || !blocks(&record) {
                return Err(invalid("only ambiguous outcomes require reconciliation"));
            }
            terminal(transaction, &mut record, state, outcome.clone())?;
            Ok(record)
        })?;
        self.wakeup.notify_waiters();
        Ok(record)
    }
    /// Full results expire after thirty days. Identity, hash, original bindings,
    /// outcome and owner fences survive for the lifetime of this database.
    pub fn compact_mutations(&self, now_unix: i64) -> Result<usize, StoreError> {
        self.transaction(|transaction| {
            let mut statement=transaction.prepare("SELECT record FROM mutation_commands WHERE terminal_at<?1 AND (state='succeeded' OR (state='failed' AND json_extract(record,'$.outcome.effect')='unapplied')) AND intent<>'' LIMIT 256")?;
            let records=statement.query_map([now_unix-30*86400],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            drop(statement);let count=records.len();
            for raw in records {
                let mut record=decode(raw)?;record.expired=true;record.params=Value::Null;record.acceptance=Value::Null;
                if let Some(value)=record.outcome.as_mut().and_then(Value::as_object_mut) {
                    value.remove("result");
                    value.remove("published");
                    value.remove("refresh");
                }
                save(transaction,&record)?;
                transaction.execute("UPDATE mutation_commands SET intent='' WHERE command_id=?1",[record.command_id])?;
            } Ok(count)
        })
    }
}
fn recover_one(
    transaction: &Transaction<'_>,
    mut record: MutationRecord,
    recovery: &mut MutationRecovery,
) -> Result<(), StoreError> {
    match record.state.as_str() {
        "executing" if record.recovery_policy == "sql_only" => {
            record.state = "accepted".into();
            record.revision += 1;
            save(transaction, &record)?;
            recovery.accepted.push(record);
        }
        "executing" => {
            terminal(
                transaction,
                &mut record,
                "failed",
                json!({"effect":"unknown","error":{"code":"outcome_unknown","message":"The process ended after effect claim","retryable":false}}),
            )?;
            recovery.terminal.push(record);
        }
        "accepted" => recovery.accepted.push(record),
        "committed" => recovery.committed.push(record),
        _ => {}
    }
    Ok(())
}
